//! HTTP/3 upstream transport (QUIC + h3), v1.0.
//!
//! Unlike [`crate::quic`] (a custom ALPN with a raw bidirectional stream),
//! this speaks real HTTP/3: the MTProto byte stream is the body of a single
//! `POST /mtproto` request, and the relay answers in the response body. That
//! makes any HTTP/3-capable edge work as a relay — nginx, Caddy, or a
//! Cloudflare Worker — without a bespoke protocol on the server side.
//!
//! `h3` exposes a request as a pair of async `send_data`/`recv_data` calls,
//! while the bridge wants `AsyncRead`/`AsyncWrite`. A `duplex` pipe bridges
//! the two: the pump task copies bytes in both directions, and the bridge
//! gets an ordinary split stream.
//!
//! Like the QUIC transport, failures are non-fatal: the caller falls through
//! to the regular CF/TCP chain, so enabling h3 without relays changes nothing.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::{Buf, Bytes};
use tokio::io::{AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};

/// ALPN for HTTP/3 (RFC 9114).
pub const H3_ALPN: &[u8] = b"h3";

/// Size of the pipe between the bridge and the HTTP/3 request body.
const DUPLEX_BUF: usize = 64 * 1024;
/// Chunk size used when copying the response body into the pipe.
const PUMP_CHUNK: usize = 32 * 1024;

static H3_CRYPTO: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
static H3_ENDPOINT: OnceLock<quinn::Endpoint> = OnceLock::new();

fn client_crypto() -> Arc<rustls::ClientConfig> {
    H3_CRYPTO
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let mut cfg = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            cfg.alpn_protocols = vec![H3_ALPN.to_vec()];
            Arc::new(cfg)
        })
        .clone()
}

fn endpoint() -> Result<quinn::Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ep) = H3_ENDPOINT.get() {
        return Ok(ep.clone());
    }
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto())?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(quic_crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    transport.max_idle_timeout(Some(Duration::from_secs(60).try_into()?));
    cfg.transport_config(Arc::new(transport));
    ep.set_default_client_config(cfg);
    let _ = H3_ENDPOINT.set(ep.clone());
    Ok(ep)
}

fn split_relay(relay: &str) -> Result<(String, u16), Box<dyn std::error::Error + Send + Sync>> {
    let relay = relay.trim();
    let (host, port) = relay
        .rsplit_once(':')
        .ok_or_else(|| format!("HTTP/3 relay must be host:port, got '{}'", relay))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("Invalid HTTP/3 relay port in '{}'", relay))?;
    if host.is_empty() {
        return Err(format!("Empty HTTP/3 relay host in '{}'", relay).into());
    }
    Ok((host.to_string(), port))
}

/// A bridged HTTP/3 session, already split into reader/writer halves.
pub struct H3Upstream {
    pub reader: ReadHalf<DuplexStream>,
    pub writer: WriteHalf<DuplexStream>,
}

/// Connect to `relay` (`host:port`) over HTTP/3 and open the streaming
/// `POST /mtproto` request that carries the MTProto byte stream.
pub async fn connect_h3(
    relay: &str,
    timeout: Duration,
) -> Result<H3Upstream, Box<dyn std::error::Error + Send + Sync>> {
    crate::quic::ensure_crypto_provider();
    let (host, port) = split_relay(relay)?;
    let addr = crate::net::resolve_host(&host, port).await?;

    let ep = endpoint()?;
    let conn = tokio::time::timeout(timeout, ep.connect(addr, &host)?)
        .await
        .map_err(|_| format!("HTTP/3 handshake to {} timed out", relay))??;

    let quic_conn = h3_quinn::Connection::new(conn);
    let (mut driver, mut send_request) = tokio::time::timeout(timeout, ::h3::client::new(quic_conn))
        .await
        .map_err(|_| format!("HTTP/3 setup to {} timed out", relay))??;

    // The driver owns the connection state; without polling it no request
    // or response frame moves. It ends when the connection closes.
    let driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });
    let driver_guard = crate::upstream::BackgroundTasks(vec![driver_task]);

    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!("https://{}/mtproto", host))
        .header("content-type", "application/octet-stream")
        .header("accept-encoding", "identity")
        .body(())?;

    let stream = tokio::time::timeout(timeout, send_request.send_request(request))
        .await
        .map_err(|_| format!("HTTP/3 request to {} timed out", relay))??;

    let (bridge_side, pump_side) = tokio::io::duplex(DUPLEX_BUF);
    tokio::spawn(async move {
        let _driver_guard = driver_guard;
        pump(pump_side, stream).await;
    });

    let (reader, writer) = tokio::io::split(bridge_side);
    Ok(H3Upstream { reader, writer })
}

/// Drive upload and response parsing independently. Some relays wait for
/// request data before sending response headers; waiting for headers before
/// reading the duplex pipe deadlocked both sides.
async fn pump<S>(pipe: DuplexStream, stream: ::h3::client::RequestStream<S, Bytes>)
where S: ::h3::quic::BidiStream<Bytes> + Send + 'static {
    use tokio::io::AsyncReadExt;
    let (mut send, mut recv) = stream.split();
    let (mut pipe_read, mut pipe_write) = tokio::io::split(pipe);
    let upload = async {
        let mut buf = vec![0u8; PUMP_CHUNK];
        loop {
            let n = pipe_read.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 { return send.finish().await.map_err(|e| e.to_string()); }
            send.send_data(Bytes::copy_from_slice(&buf[..n])).await.map_err(|e| e.to_string())?;
        }
    };
    let download = async {
        let response = tokio::time::timeout(Duration::from_secs(20), recv.recv_response())
            .await.map_err(|_| "HTTP/3 response headers timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() { return Err(format!("HTTP/3 relay returned {}", response.status())); }
        while let Some(mut data) = recv.recv_data().await.map_err(|e| e.to_string())? {
            while data.has_remaining() {
                let chunk = data.chunk();
                pipe_write.write_all(chunk).await.map_err(|e| e.to_string())?;
                let len = chunk.len();
                data.advance(len);
            }
        }
        pipe_write.shutdown().await.map_err(|e| e.to_string())
    };
    tokio::pin!(upload, download);
    let result = tokio::select! {
        result = &mut upload => match result { Ok(()) => download.await, error => error },
        result = &mut download => result,
    };
    if let Err(error) = result { tracing::debug!("HTTP/3 pump stopped: {}", error); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn relay_string_validation() {
        assert!(split_relay("example.com:443").is_ok());
        assert!(split_relay("no-port").is_err());
        assert!(split_relay(":443").is_err());
        assert!(split_relay("host:notaport").is_err());
    }

    /// Full roundtrip over a local HTTP/3 server: request body in, response
    /// body out, through the duplex bridge the proxy actually uses.
    #[tokio::test]
    async fn h3_roundtrip_echoes_body() { h3_roundtrip(false).await; }

    #[tokio::test]
    async fn h3_relay_can_wait_for_request_body_before_headers() { h3_roundtrip(true).await; }

    async fn h3_roundtrip(delayed_headers: bool) {
        crate::quic::ensure_crypto_provider();

        let (cert_der, key_der) = self_signed();
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert_der.clone())],
                rustls::pki_types::PrivateKeyDer::try_from(key_der).unwrap(),
            )
            .unwrap();
        server_crypto.alpn_protocols = vec![H3_ALPN.to_vec()];

        let quic_server = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(server_crypto))
            .expect("quic server crypto");
        let server_ep = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(quic_server)),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let server_addr = server_ep.local_addr().unwrap();

        // Minimal relay: echo every request body chunk back in the response.
        tokio::spawn(async move {
            let conn = server_ep.accept().await.unwrap().await.unwrap();
            let mut h3_conn =
                ::h3::server::Connection::new(h3_quinn::Connection::new(conn)).await.unwrap();
            while let Ok(Some(resolver)) = h3_conn.accept().await {
                tokio::spawn(async move {
                    let (_, mut stream) = match resolver.resolve_request().await {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    let resp = http::Response::builder().status(200).body(()).unwrap();
                    if !delayed_headers && stream.send_response(resp).await.is_err() {
                        return;
                    }
                    let mut needs_headers = delayed_headers;
                    while let Ok(Some(mut chunk)) = stream.recv_data().await {
                        if needs_headers {
                            let response = http::Response::builder().status(200).body(()).unwrap();
                            if stream.send_response(response).await.is_err() { return; }
                            needs_headers = false;
                        }
                        let mut data = Vec::with_capacity(chunk.remaining());
                        while chunk.has_remaining() {
                            let piece = chunk.chunk().to_vec();
                            chunk.advance(piece.len());
                            data.extend_from_slice(&piece);
                        }
                        if stream.send_data(Bytes::from(data)).await.is_err() {
                            return;
                        }
                    }
                    let _ = stream.finish().await;
                });
            }
        });

        // Client side, using a root store that trusts our test certificate.
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(cert_der))
            .unwrap();
        let mut client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![H3_ALPN.to_vec()];

        let mut ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let quic_crypto =
            quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(client_crypto)).unwrap();
        let mut cfg = quinn::ClientConfig::new(Arc::new(quic_crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
        cfg.transport_config(Arc::new(transport));
        ep.set_default_client_config(cfg);

        let conn = ep.connect(server_addr, "127.0.0.1").unwrap().await.unwrap();
        let (mut driver, mut send_request) =
            ::h3::client::new(h3_quinn::Connection::new(conn)).await.unwrap();
        tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

        let req = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://127.0.0.1/mtproto")
            .body(())
            .unwrap();
        let stream = send_request.send_request(req).await.unwrap();

        let (bridge_side, pump_side) = tokio::io::duplex(DUPLEX_BUF);
        tokio::spawn(pump(pump_side, stream));

        let (mut reader, mut writer) = tokio::io::split(bridge_side);
        let payload = b"mtproto-bytes-over-h3".to_vec();
        writer.write_all(&payload).await.unwrap();

        let mut echo = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), reader.read_exact(&mut echo))
            .await
            .expect("echo timed out")
            .expect("echo read failed");
        assert_eq!(echo, payload);
    }

    fn self_signed() -> (Vec<u8>, Vec<u8>) {
        let params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        (cert.der().to_vec(), key_pair.serialize_der())
    }
}
