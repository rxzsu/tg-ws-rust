use std::collections::HashMap;
use std::time::Duration;
use std::sync::Arc;
use tokio::sync::RwLock;
use rand::prelude::IndexedRandom;
use rand::RngExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;

const CFPROXY_ENC: &[&str] = &[
    "virkgj.com", "vmmzovy.com", "mkuosckvso.com", "zaewayzmplad.com", "twdmbzcm.com",
    "awzwsldi.com", "clngqrflngqin.com", "tjacxbqtj.com", "bxaxtxmrw.com", "dmohrsgmohcrwb.com",
    "vwbmtmoi.com", "khgrre.com", "ulihssf.com", "tmhqsdqmfpmk.com", "xwuwoqbm.com",
    "orgcnunpj.com", "zhkuldz.com", "zypoljnslxa.com", "efabnxaowuzs.com", "zaftuzsftqdq.com"
];

pub fn decode_cf_domain(s: &str) -> String {
    if !s.ends_with(".com") {
        return s.to_string();
    }
    let p = &s[..s.len() - 4];
    let n = p.chars().filter(|c| c.is_ascii_alphabetic()).count() as i32;
    let mut decoded = String::with_capacity(s.len() + 6);
    for c in p.chars() {
        if c.is_ascii_alphabetic() {
            let base = if c.is_ascii_lowercase() { b'a' } else { b'A' } as i32;
            let val = (c as i32) - base - n;
            let shifted = val.rem_euclid(26);
            decoded.push(((base + shifted) as u8) as char);
        } else {
            decoded.push(c);
        }
    }
    decoded.push_str(".co.uk");
    decoded
}

pub fn get_default_cf_domains() -> Vec<String> {
    CFPROXY_ENC.iter().map(|s| decode_cf_domain(s)).collect()
}

#[derive(Clone)]
pub struct Balancer {
    domains: Arc<RwLock<Vec<String>>>,
    dc_to_domain: Arc<RwLock<HashMap<i32, String>>>,
    /// Measured health per balancer domain (RTT in µs, None = failed probe).
    health: Arc<RwLock<HashMap<String, Option<u128>>>>,
    /// When false, neither the built-in third-party domain pool nor its
    /// GitHub refresh loop is used (privacy opt-out).
    builtin_enabled: bool,
}

impl Default for Balancer {
    fn default() -> Self {
        Self::new()
    }
}

impl Balancer {
    pub fn new() -> Self {
        Self::with_builtins(true)
    }

    pub fn with_builtins(enabled: bool) -> Self {
        let defaults = if enabled {
            get_default_cf_domains()
        } else {
            Vec::new()
        };
        let mut dc_map = HashMap::new();
        let mut rng = rand::rng();
        for &dc in &[1, 2, 3, 4, 5, 203] {
            if let Some(d) = defaults.as_slice().choose(&mut rng) {
                dc_map.insert(dc, d.clone());
            }
        }
        Self {
            domains: Arc::new(RwLock::new(defaults)),
            dc_to_domain: Arc::new(RwLock::new(dc_map)),
            health: Arc::new(RwLock::new(HashMap::new())),
            builtin_enabled: enabled,
        }
    }

    pub async fn update_domains_list(&self, domains_list: Vec<String>) {
        if domains_list.is_empty() {
            return;
        }
        let mut d_guard = self.domains.write().await;
        *d_guard = domains_list.clone();

        let mut dc_guard = self.dc_to_domain.write().await;
        let mut rng = rand::rng();
        for &dc in &[1, 2, 3, 4, 5, 203] {
            if let Some(d) = d_guard.as_slice().choose(&mut rng) {
                dc_guard.insert(dc, d.clone());
            }
        }
    }

    /// Domains for a DC ordered by measured health: fastest first, then
    /// unknown (never probed) domains, then dead ones. Falls back to random
    /// order inside each tier so we still spread the load.
    pub async fn get_domains_for_dc(&self, dc: i32) -> Vec<String> {
        let d_guard = self.domains.read().await;
        let health = self.health.read().await;

        let mut scored: Vec<(u8, u64, String)> = d_guard
            .iter()
            .map(|d| {
                let (tier, rtt) = match health.get(d) {
                    // Measured healthy: ordered by RTT, fastest first.
                    Some(Some(us)) => (0u8, (*us).min(u64::MAX as u128) as u64),
                    // Never probed: still worth trying, but behind known-good.
                    None => (1, rand::rng().random_range(0..1_000_000)),
                    // Explicitly failed: last resort.
                    Some(None) => (2, 0),
                };
                (tier, rtt, d.clone())
            })
            .collect();
        drop(d_guard);
        drop(health);

        scored.sort_by_key(|(tier, rtt, _)| (*tier, *rtt));

        let mut list: Vec<String> = scored.into_iter().map(|(_, _, d)| d).collect();

        // Current domain for this DC stays first: switching costs a handshake.
        let current = self.dc_to_domain.read().await.get(&dc).cloned();
        if let Some(c) = current {
            if let Some(pos) = list.iter().position(|d| *d == c) {
                list.remove(pos);
            }
            list.insert(0, c);
        }
        list
    }

    /// Pick the next domain for a DC, preferring healthy ones.
    pub async fn rotate_domain_for_dc(&self, dc: i32) {
        let ordered = self.get_domains_for_dc(dc).await;
        let mut dc_guard = self.dc_to_domain.write().await;
        // Rotate past the current pick into the health-ordered list.
        let current = dc_guard.get(&dc).cloned();
        let candidates: Vec<_> = ordered
            .into_iter()
            .filter(|d| Some(d) != current.as_ref())
            .collect();
        if let Some(chosen) = candidates.first() {
            dc_guard.insert(dc, chosen.clone());
        }
    }

    /// Record a successful connect with its measured RTT (TLS handshake +
    /// WS upgrade). Overrides both failures and stale probe data.
    pub async fn record_success(&self, domain: &str, rtt_us: u128) {
        let mut health = self.health.write().await;
        health.insert(domain.to_string(), Some(rtt_us));
    }

    /// Mark a domain as failed right now (connection refused, RST, timeout,
    /// TLS error). Pushes it to the back of the health ordering immediately
    /// instead of waiting for the next probe round.
    pub async fn record_failure(&self, domain: &str) {
        let mut health = self.health.write().await;
        health.insert(domain.to_string(), None);
    }

    /// One probe round: TLS handshake + WS upgrade to every domain, record
    /// RTT. Runs in the background on a timer; results feed the ordering in
    /// `get_domains_for_dc`.
    async fn probe_all(&self) {
        let domains = self.domains.read().await.clone();
        let mut handles = Vec::with_capacity(domains.len());
        for d in domains {
            handles.push(tokio::spawn(async move {
                // Telegram exposes every DC as `kws{dc}.<domain>`. Measure the
                // two busiest DCs and keep the best RTT as the domain score.
                let mut best: Option<u128> = None;
                for dc in [2u8, 4u8] {
                    let host = format!("kws{}.{}", dc, d);
                    let start = std::time::Instant::now();
                    let res = tokio::time::timeout(
                        std::time::Duration::from_secs(6),
                        crate::raw_websocket::RawWebSocket::connect(&host, &host, "/apiws", true),
                    )
                    .await;
                    if let Ok(Ok(mut ws)) = res {
                        let _ = ws.close().await;
                        let rtt = start.elapsed().as_micros();
                        best = Some(best.map_or(rtt, |b| b.min(rtt)));
                    }
                }
                (d, best)
            }));
        }

        let mut results = HashMap::new();
        for h in handles {
            if let Ok((d, rtt)) = h.await {
                results.insert(d, rtt);
            }
        }
        if results.is_empty() {
            return;
        }
        let mut health = self.health.write().await;
        // Drop domains that vanished from the list, then merge this round.
        health.retain(|d, _| results.contains_key(d));
        for (d, rtt) in results {
            health.insert(d, rtt);
        }
    }

    /// Background loop: hourly GitHub domain refresh + health probes every
    /// 5 minutes (first probe runs shortly after startup).
    pub fn start_background_refresh(self: Arc<Self>) {
        // Health probe loop.
        let probe = self.clone();
        tokio::spawn(async move {
            // Small settle delay so startup is not delayed by probes.
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            loop {
                probe.probe_all().await;
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            }
        });
        // GitHub refresh loop (built-in third-party pool only).
        if !self.builtin_enabled {
            return;
        }
        tokio::spawn(async move {
            loop {
                if let Ok(domains) = fetch_github_cf_domains().await
                    && domains.len() >= 3
                {
                    self.update_domains_list(domains).await;
                    tracing::info!("Cloudflare balancer domain pool refreshed from GitHub");
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    }
}

const GITHUB_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GITHUB_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Cap on the response body of one GitHub fetch: the domain list is a few KB
/// and the release JSON is tens of KB; anything larger is garbage.
const GITHUB_BODY_CAP: usize = 2 * 1024 * 1024;

async fn fetch_github_cf_domains() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    // Bounded: a hung GitHub connection must not stall the hourly refresh
    // loop forever (the loop sleeps only after the fetch returns).
    let tcp = tokio::time::timeout(GITHUB_CONNECT_TIMEOUT, TcpStream::connect("raw.githubusercontent.com:443")).await??;
    let server_name = ServerName::try_from("raw.githubusercontent.com".to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut tls = connector.connect(server_name, tcp).await?;

    let request = "GET /Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt HTTP/1.1\r\n\
                   Host: raw.githubusercontent.com\r\n\
                   User-Agent: tg-ws-proxy\r\n\
                   Connection: close\r\n\r\n";

    tls.write_all(request.as_bytes()).await?;
    tls.flush().await?;

    // Real response parsing: status check + chunked/length body decode, all
    // bounded. Previously `read_to_end` + a split at `\r\n\r\n` fed a 403
    // page or chunked framing hex straight into the domain parser.
    let resp = tokio::time::timeout(
        GITHUB_READ_TIMEOUT,
        crate::mini_http::read_response(&mut tls, GITHUB_BODY_CAP),
    )
    .await
    .map_err(|_| "GitHub domains fetch timed out".to_string())??;
    if resp.status != 200 {
        return Err(format!("GitHub domains fetch returned HTTP {}", resp.status).into());
    }

    let body = String::from_utf8_lossy(&resp.body);
    let mut list = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let decoded = decode_cf_domain(trimmed);
        if !decoded.is_empty() && !list.contains(&decoded) {
            list.push(decoded);
        }
    }
    Ok(list)
}

pub async fn check_latest_github_release() -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = tokio::time::timeout(GITHUB_CONNECT_TIMEOUT, TcpStream::connect("api.github.com:443")).await??;
    let _ = tcp.set_nodelay(true);

    let server_name = ServerName::try_from("api.github.com".to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut tls = connector.connect(server_name, tcp).await?;
    let req = "GET /repos/rxzsu/tg-ws-rust/releases/latest HTTP/1.1\r\n\
               Host: api.github.com\r\n\
               User-Agent: tg-ws-proxy\r\n\
               Accept: application/vnd.github+json\r\n\
               Connection: close\r\n\r\n";

    tls.write_all(req.as_bytes()).await?;
    tls.flush().await?;

    // Same bounded, status-checked parsing as the domains fetch: a 403
    // rate-limit JSON (no `tag_name`) must surface as an error, not silently
    // look like "no updates".
    let resp = tokio::time::timeout(
        GITHUB_READ_TIMEOUT,
        crate::mini_http::read_response(&mut tls, GITHUB_BODY_CAP),
    )
    .await
    .map_err(|_| "GitHub release fetch timed out".to_string())??;
    if resp.status != 200 {
        return Err(format!("GitHub release fetch returned HTTP {}", resp.status).into());
    }

    let body = String::from_utf8_lossy(&resp.body);
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("invalid release JSON: {e}"))?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if tag.is_empty() {
        return Err("release response has no tag_name".into());
    }
    let url = v.get("html_url").and_then(|u| u.as_str()).unwrap_or("https://github.com/rxzsu/tg-ws-rust/releases").to_string();
    Ok((tag, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Healthy domains (fastest first) → unknown → failed.
    #[tokio::test]
    async fn health_ordering_puts_failures_last() {
        let b = Balancer::new();
        b.update_domains_list(vec![
            "a.test".to_string(),
            "b.test".to_string(),
            "c.test".to_string(),
            "d.test".to_string(),
        ])
        .await;

        b.record_success("b.test", 1_000).await;
        b.record_success("a.test", 50_000).await;
        b.record_failure("c.test").await;

        // DC 9 is not in the sticky map, so the ordering is pure health.
        let order = b.get_domains_for_dc(9).await;
        assert_eq!(order[0], "b.test", "fastest healthy domain first");
        assert_eq!(order[1], "a.test", "slower healthy domain second");
        assert_eq!(order[2], "d.test", "unknown domain before the dead one");
        assert_eq!(order[3], "c.test", "failed domain last");
    }
}
