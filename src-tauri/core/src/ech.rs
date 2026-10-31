//! Encrypted Client Hello (ECH) support for upstream TLS connections.
//!
//! DPI can read the plain `server_name` extension of a TLS ClientHello and
//! selectively throttle it even when the IP is fine. ECH encrypts the whole
//! sensitive part of the ClientHello with a public key the server publishes
//! in its DNS HTTPS record (Cloudflare fronts everything we connect to, and
//! supports ECH on all proxied zones).
//!
//! Flow per domain:
//! 1. DoH (JSON) query for the HTTPS (type 65) record → extract the `ech=`
//!    SvcParam (base64 ECHConfigList).
//! 2. Build a dedicated `ClientConfig` via rustls `with_ech`, HPKE provided
//!    by the aws-lc-rs provider that is already compiled into the tree.
//! 3. Cache the result (positive 1h, negative 10 min) so steady-state
//!    connections pay zero DNS overhead.
//!
//! Failure semantics: no config / lookup failure → plain TLS; handshake with
//! ECH rejected by the server → the caller retries with plain TLS.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::{EchConfigListBytes, ServerName};
use tokio_rustls::TlsConnector;
use base64::Engine as _;

static ECH_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ECH_ENABLED.store(on, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ECH_ENABLED.load(Ordering::Relaxed)
}

/// How long a successfully fetched ECH config stays valid (configs rotate
/// slowly — Cloudflare publishes new keys infrequently).
const POSITIVE_TTL: Duration = Duration::from_secs(3600);
/// A domain without ECH (or with unreachable DoH) is retried after this.
const NEGATIVE_TTL: Duration = Duration::from_secs(600);
/// Whole DoH query budget; the caller already races parallel connects, so a
/// slow lookup must not stall the winner selection for long.
const DOH_TIMEOUT: Duration = Duration::from_secs(4);

type EchClientConfig = Arc<ClientConfig>;

#[derive(Clone)]
struct CacheEntry {
    config: Option<EchClientConfig>,
    at: Instant,
}

static CACHE: Mutex<Option<HashMap<String, CacheEntry>>> = Mutex::const_new(None);

/// Resolve (with cache) an ECH-enabled TLS config for `domain`.
/// `None` means "no ECH available, use the plain config".
///
/// The cache mutex is held only for the map lookups — never across the DoH
/// fetch. Holding it during `fetch_ech_config` (up to ~24 s across two
/// resolvers) serialized *every* upstream connect behind one lookup, turning
/// the parallel transport race into a serial crawl and burning each
/// candidate's `connect_timeout` while it waited for the lock.
pub async fn tls_config_for(domain: &str) -> Option<EchClientConfig> {
    if !is_enabled() {
        return None;
    }
    {
        let cache_guard = CACHE.lock().await;
        if let Some(cache) = cache_guard.as_ref()
            && let Some(entry) = cache.get(domain)
        {
            let ttl = if entry.config.is_some() { POSITIVE_TTL } else { NEGATIVE_TTL };
            if entry.at.elapsed() < ttl {
                return entry.config.clone();
            }
        }
    }

    // Fetch outside the lock. A thundering herd after TTL expiry may fetch
    // the same domain twice; both insert the same answer and the cost is a
    // duplicate DoH query, not a stall.
    let config = match fetch_ech_config(domain).await {
        Ok(list) => match build_ech_client_config(&list) {
            Ok(cfg) => Some(Arc::new(cfg)),
            Err(e) => {
                tracing::debug!("ECH config for {} is unusable: {}", domain, e);
                None
            }
        },
        Err(e) => {
            tracing::debug!("ECH lookup failed for {}: {}", domain, e);
            None
        }
    };

    CACHE
        .lock()
        .await
        .get_or_insert_with(HashMap::new)
        .insert(domain.to_string(), CacheEntry { config: config.clone(), at: Instant::now() });
    config
}

/// DoH (JSON API) lookup of the HTTPS record set, extracting the base64
/// `ech` SvcParam. Queries go to well-known resolvers by IP (SNI kept for
/// certificate validity), so no system resolver is involved.
async fn fetch_ech_config(domain: &str) -> Result<Vec<u8>, String> {
    for (ip, sni) in [("1.1.1.1", "cloudflare-dns.com"), ("8.8.8.8", "dns.google")] {
        match doh_query(ip, sni, domain).await {
            Ok(list) if !list.is_empty() => return Ok(list),
            Ok(_) => continue,
            Err(e) => tracing::debug!("DoH {} ({}) failed: {}", ip, sni, e),
        }
    }
    Err("all DoH resolvers failed".into())
}

async fn doh_query(ip: &str, sni: &str, domain: &str) -> Result<Vec<u8>, String> {
    let tcp = tokio::time::timeout(
        DOH_TIMEOUT,
        TcpStream::connect(format!("{ip}:443")),
    )
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);

    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let server_name = ServerName::try_from(sni.to_string())
        .map_err(|e| e.to_string())?;
    let mut tls = tokio::time::timeout(DOH_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .map_err(|_| "tls timeout".to_string())?
        .map_err(|e| e.to_string())?;

    // JSON API: `Answer[].data` holds the presentation-format record.
    let req = format!(
        "GET /dns-query?name={}&type=HTTPS HTTP/1.1\r\n\
         Host: {}\r\n\
         Accept: application/dns-json\r\n\
         User-Agent: tg-ws-proxy\r\n\
         Connection: close\r\n\r\n",
        domain, sni
    );
    tls.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    tls.flush().await.map_err(|e| e.to_string())?;

    let mut resp = Vec::new();
    // Bounded read; the body is parsed through the shared mini-HTTP parser so
    // a chunked DoH answer (Cloudflare does this) lands in the JSON extractor
    // decoded, not with hex sizes inline.
    tokio::time::timeout(DOH_TIMEOUT, tls.read_to_end(&mut resp))
        .await
        .map_err(|_| "read timeout".to_string())?
        .map_err(|e| e.to_string())?;

    let parsed = crate::mini_http::parse_response(&resp, 256 * 1024)?;
    if parsed.status != 200 {
        return Err(format!("DoH resolver returned HTTP {}", parsed.status));
    }
    let body = String::from_utf8_lossy(&parsed.body);
    extract_ech_param(&body)
}

/// Pull the `ech=` base64 value out of the DoH JSON body without a full
/// JSON-schema model: the interesting values live inside the raw
/// presentation-format `data` strings.
fn extract_ech_param(body: &str) -> Result<Vec<u8>, String> {
    let idx = body.find("ech=").ok_or("no ech param in DoH answer")?;
    let rest = &body[idx + 4..];
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest.find([' ', '"', '}']).unwrap_or(rest.len());
    let value = rest[..end].trim_end_matches(['"', ' ', ',']);
    if value.is_empty() {
        return Err("empty ech param".into());
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value)
        .or_else(|_| {
            // Some resolvers pad or use the URL-safe alphabet.
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value)
        })
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
        .map_err(|e| format!("bad ech base64: {e}"))
}

/// Build a rustls client config that speaks ECH with the given config list.
/// TLS 1.3-only (an ECH requirement) and provider-pinned to aws-lc-rs
/// (the only provider in the tree with HPKE).
fn build_ech_client_config(list: &[u8]) -> Result<ClientConfig, String> {
    let ech_config = tokio_rustls::rustls::client::EchConfig::new(
        EchConfigListBytes::from(list.to_vec()),
        tokio_rustls::rustls::crypto::aws_lc_rs::hpke::ALL_SUPPORTED_SUITES,
    )
    .map_err(|e| e.to_string())?;

    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    Ok(
        ClientConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_ech(ech_config.into())
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ech_from_doh_body() {
        let body = r#"{"Status":0,"Answer":[{"name":"example.com.","type":64,"TTL":300,"data":"1 36 alpn=\"h3,h2\" ech=MTESTDAT ipv4hint=1.2.3.4"}]}"#;
        let decoded = extract_ech_param(body).unwrap();
        assert_eq!(
            decoded,
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode("MTESTDAT")
                .unwrap()
        );
    }

    #[test]
    fn errors_without_ech() {
        let body = r#"{"Status":0,"Answer":[{"name":"example.com.","type":64,"TTL":300,"data":"1 36 alpn=\"h3,h2\""}]}"#;
        assert!(extract_ech_param(body).is_err());
    }
}
