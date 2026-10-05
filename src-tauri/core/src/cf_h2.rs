use std::sync::Arc;
use bytes::Bytes;
use http::{Method, Request};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use tracing::debug;

pub struct CfH2Pool {
    client_config: Arc<ClientConfig>,
}

impl Default for CfH2Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl CfH2Pool {
    pub fn new() -> Self {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];

        Self {
            client_config: Arc::new(config),
        }
    }

    pub async fn post_packet(
        &self,
        domain: &str,
        packet: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let connector = TlsConnector::from(self.client_config.clone());
        let addr = format!("{}:443", domain);
        let tcp = TcpStream::connect(addr).await?;
        let _ = tcp.set_nodelay(true);

        let server_name = ServerName::try_from(domain.to_string())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let tls = connector.connect(server_name, tcp).await?;
        let (mut client, h2_conn) = h2::client::Builder::new().handshake(tls).await?;

        tokio::spawn(async move {
            if let Err(e) = h2_conn.await {
                debug!("H2 connection drive closed: {:?}", e);
            }
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("https://{}/api", domain))
            .header("content-type", "application/octet-stream")
            .header("accept-encoding", "identity")
            .body(())?;

        let (response, mut send_stream) = client.send_request(request, false)?;
        send_stream.send_data(Bytes::copy_from_slice(packet), true)?;

        let (head, mut body) = response.await?.into_parts();
        if head.status != http::StatusCode::OK {
            return Err(format!("H2 returned HTTP status {}", head.status).into());
        }

        let mut response_data = Vec::new();
        while let Some(chunk) = body.data().await {
            let data = chunk?;
            response_data.extend_from_slice(&data);
        }

        Ok(response_data)
    }
}
