//! Experimental QUIC upstream transport (v1.0 phase 3).
//!
//! Honest scope: Telegram DCs and Cloudflare Workers do not speak QUIC for
//! MTProto relay, so this transport targets **user-run QUIC relays** (UDP)
//! that forward the stream to Telegram. Both ends must agree on [`QUIC_ALPN`].
//!
//! Attempts fail fast (configurable timeout); on failure the connection
//! falls through to the regular CF/TCP fallback chain, so enabling QUIC
//! without relays changes nothing.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// ALPN negotiated with QUIC relays. Must match on both ends.
pub const QUIC_ALPN: &[u8] = b"tg-ws-proxy-quic-v1";

/// Shared client endpoint (one UDP socket for all QUIC connections).
static CLIENT_ENDPOINT: OnceLock<quinn::Endpoint> = OnceLock::new();

/// Shared rustls client config with system roots (session tickets reused
/// across connections, same idea as the WS path).
static CLIENT_CRYPTO: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
/// QUIC needs exactly one process-wide rustls crypto provider. Our tree
/// can contain both `ring` (quinn/tokio-rustls) and `aws-lc-rs` (rcgen in
/// tests), which makes auto-detection panic — so pin `ring` explicitly.
pub(crate) fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider(),
        );
    });
}

fn client_crypto() -> Arc<rustls::ClientConfig> {
    CLIENT_CRYPTO
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let mut cfg = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            cfg.alpn_protocols = vec![QUIC_ALPN.to_vec()];
            Arc::new(cfg)
        })
        .clone()
}

fn quinn_client_config(
    crypto: Arc<rustls::ClientConfig>,
) -> Result<quinn::ClientConfig, Box<dyn std::error::Error + Send + Sync>> {
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?;
    Ok(quinn::ClientConfig::new(Arc::new(quic_crypto)))
}

fn client_endpoint() -> Result<quinn::Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ep) = CLIENT_ENDPOINT.get() {
        return Ok(ep.clone());
    }
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(quinn_client_config(client_crypto())?);
    let _ = CLIENT_ENDPOINT.set(endpoint.clone());
    Ok(endpoint)
}

pub struct QuicUpstream {
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
    pub remote: SocketAddr,
}

/// Parse `host:port` relay strings (DNS resolved at connect time).
fn split_relay(relay: &str) -> Result<(String, u16), Box<dyn std::error::Error + Send + Sync>> {
    let relay = relay.trim();
    let (host, port) = relay
        .rsplit_once(':')
        .ok_or_else(|| format!("QUIC relay must be host:port, got '{}'", relay))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("Invalid QUIC relay port in '{}'", relay))?;
    if host.is_empty() {
        return Err(format!("Empty QUIC relay host in '{}'", relay).into());
    }
    Ok((host.to_string(), port))
}

/// Connect to a QUIC relay with a custom rustls config (used by tests with
/// self-signed certs; production uses [`connect_quic`] with system roots).
pub async fn connect_quic_with_crypto(
    relay: &str,
    timeout: Duration,
    crypto: Arc<rustls::ClientConfig>,
) -> Result<QuicUpstream, Box<dyn std::error::Error + Send + Sync>> {
    ensure_crypto_provider();
    let (host, port) = split_relay(relay)?;
    let addr: SocketAddr = crate::net::resolve_host(&host, port).await?;

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    let mut client_cfg = quinn_client_config(crypto)?;
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    transport.max_idle_timeout(Some(Duration::from_secs(30).try_into()?));
    client_cfg.transport_config(Arc::new(transport));
    endpoint.set_default_client_config(client_cfg);

    let connecting = endpoint
        .connect(addr, &host)
        .map_err(|e| format!("QUIC connect to {} failed: {:?}", relay, e))?;
    let conn = tokio::time::timeout(timeout, connecting)
        .await
        .map_err(|_| format!("QUIC handshake to {} timed out", relay))??;

    let (send, recv) = tokio::time::timeout(timeout, conn.open_bi())
        .await
        .map_err(|_| format!("QUIC stream open to {} timed out", relay))??;

    Ok(QuicUpstream {
        send,
        recv,
        remote: addr,
    })
}

/// Connect to a QUIC relay using system roots and [`QUIC_ALPN`].
pub async fn connect_quic(
    relay: &str,
    timeout: Duration,
) -> Result<QuicUpstream, Box<dyn std::error::Error + Send + Sync>> {
    ensure_crypto_provider();
    let (host, port) = split_relay(relay)?;
    let addr: SocketAddr = crate::net::resolve_host(&host, port).await?;

    let endpoint = client_endpoint()?;
    let connecting = endpoint
        .connect(addr, &host)
        .map_err(|e| format!("QUIC connect to {} failed: {:?}", relay, e))?;
    let conn = tokio::time::timeout(timeout, connecting)
        .await
        .map_err(|_| format!("QUIC handshake to {} timed out", relay))??;

    let (send, recv) = tokio::time::timeout(timeout, conn.open_bi())
        .await
        .map_err(|_| format!("QUIC stream open to {} timed out", relay))??;

    Ok(QuicUpstream {
        send,
        recv,
        remote: addr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_crypto(cert_der: &[u8]) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(cert_der.to_vec()))
            .unwrap();
        let mut cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        cfg.alpn_protocols = vec![QUIC_ALPN.to_vec()];
        Arc::new(cfg)
    }

    /// Full roundtrip against an in-process QUIC relay: handshake, ALPN,
    /// bidirectional stream echo. Proves the transport works end to end.
    #[tokio::test]
    async fn quic_relay_roundtrip() {
        ensure_crypto_provider();
        let Certified { cert_der, key_der } = self_signed_cert();

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert_der.clone())],
                rustls::pki_types::PrivateKeyDer::try_from(key_der).unwrap(),
            )
            .unwrap();
        server_crypto.alpn_protocols = vec![QUIC_ALPN.to_vec()];
        let quic_server = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(server_crypto))
            .expect("quic server crypto");
        let server_ep = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(quic_server)),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let server_addr = server_ep.local_addr().unwrap();

        // Minimal relay: echo one message back, then stay alive until the
        // test is done (dropping the endpoint would close the connection
        // before the client reads the echo).
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let conn = server_ep.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let mut buf = vec![0u8; 64];
            recv.read_exact(&mut buf).await.unwrap();
            send.write_all(&buf).await.unwrap();
            send.finish().unwrap();
            let _ = done_rx.await;
        });

        let relay = format!("127.0.0.1:{}", server_addr.port());
        let mut up = connect_quic_with_crypto(&relay, Duration::from_secs(5), test_crypto(&cert_der))
            .await
            .expect("quic connect failed");

        let payload = [0xABu8; 64];
        up.send.write_all(&payload).await.unwrap();
        up.send.finish().unwrap();
        let mut echo = vec![0u8; 64];
        up.recv.read_exact(&mut echo).await.unwrap();
        assert_eq!(echo, payload);
        let _ = done_tx.send(());
    }

    struct Certified {
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
    }

    fn self_signed_cert() -> Certified {
        let params =
            rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        Certified {
            cert_der: cert.der().to_vec(),
            key_der: key_pair.serialize_der(),
        }
    }

    #[test]
    fn relay_string_validation() {
        assert!(split_relay("example.com:443").is_ok());
        assert!(split_relay("no-port").is_err());
        assert!(split_relay(":443").is_err());
        assert!(split_relay("host:notaport").is_err());
    }
}
