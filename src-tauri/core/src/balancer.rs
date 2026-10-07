use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use rand::seq::SliceRandom;
use rand::prelude::IndexedRandom;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
}

impl Default for Balancer {
    fn default() -> Self {
        Self::new()
    }
}

impl Balancer {
    pub fn new() -> Self {
        let defaults = get_default_cf_domains();
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

    pub async fn get_domains_for_dc(&self, dc: i32) -> Vec<String> {
        let dc_guard = self.dc_to_domain.read().await;
        let d_guard = self.domains.read().await;

        let current = dc_guard.get(&dc).cloned();
        let mut list = Vec::new();
        if let Some(c) = current.clone() {
            list.push(c);
        }

        let mut shuffled = d_guard.clone();
        shuffled.shuffle(&mut rand::rng());
        for d in shuffled {
            if Some(&d) != current.as_ref() {
                list.push(d);
            }
        }
        list
    }

    pub async fn rotate_domain_for_dc(&self, dc: i32) {
        let d_guard = self.domains.read().await;
        let mut dc_guard = self.dc_to_domain.write().await;

        let current = dc_guard.get(&dc);
        let alternatives: Vec<_> = d_guard.iter().filter(|d| Some(*d) != current).collect();
        if let Some(chosen) = alternatives.as_slice().choose(&mut rand::rng()) {
            dc_guard.insert(dc, (*chosen).clone());
        }
    }

    pub fn start_background_refresh(self: Arc<Self>) {
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

async fn fetch_github_cf_domains() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect("raw.githubusercontent.com:443").await?;
    let server_name = ServerName::try_from("raw.githubusercontent.com".to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut tls = connector.connect(server_name, tcp).await?;

    let request = "GET /Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt HTTP/1.1\r\n\
                   Host: raw.githubusercontent.com\r\n\
                   User-Agent: tg-ws-proxy\r\n\
                   Connection: close\r\n\r\n";

    tls.write_all(request.as_bytes()).await?;
    tls.flush().await?;

    let mut response = Vec::new();
    tls.read_to_end(&mut response).await?;

    if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
        let body = String::from_utf8_lossy(&response[pos + 4..]);
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
        return Ok(list);
    }

    Err("Invalid HTTP response from GitHub".into())
}

pub async fn check_latest_github_release() -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect("api.github.com:443").await?;
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

    let mut resp = Vec::new();
    tls.read_to_end(&mut resp).await?;

    if let Some(pos) = resp.windows(4).position(|w| w == b"\r\n\r\n") {
        let body = String::from_utf8_lossy(&resp[pos + 4..]);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let url = v.get("html_url").and_then(|u| u.as_str()).unwrap_or("https://github.com/rxzsu/tg-ws-rust/releases").to_string();
            return Ok((tag, url));
        }
    }

    Err("Failed to parse GitHub release response".into())
}
