use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use rand::RngExt;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

pub const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

pub enum WsStream {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
}

impl tokio::io::AsyncRead for WsStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            WsStream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for WsStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            WsStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            WsStream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            WsStream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            WsStream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

fn xor_mask(data: &mut [u8], mask: &[u8; 4]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
}

pub struct RawWebSocket {
    stream: WsStream,
    leftover: Vec<u8>,
}

pub struct WsReader {
    reader: ReadHalf<WsStream>,
    closed: bool,
    frag: Vec<u8>,
    leftover: Vec<u8>,
}

pub struct WsWriter {
    writer: WriteHalf<WsStream>,
    closed: bool,
}

impl RawWebSocket {
    pub async fn connect(
        host: &str,
        domain: &str,
        path: &str,
        secure: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let stream = if secure {
            let addr = format!("{}:443", host);
            let tcp = TcpStream::connect(addr).await?;
            let _ = tcp.set_nodelay(true);

            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

            let config = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();

            let connector = TlsConnector::from(Arc::new(config));
            let server_name = ServerName::try_from(domain.to_string())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

            let tls = connector.connect(server_name, tcp).await?;
            WsStream::Tls(tls)
        } else {
            let addr = format!("{}:80", host);
            let tcp = TcpStream::connect(addr).await?;
            let _ = tcp.set_nodelay(true);
            WsStream::Plain(tcp)
        };

        let mut ws = Self { stream, leftover: Vec::new() };

        // Handshake
        let mut key_bytes = [0u8; 16];
        rand::rng().fill(&mut key_bytes);
        let ws_key = BASE64.encode(key_bytes);

        let req = format!(
            "GET {} HTTP/1.1\r\n\
             Host: {}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: binary\r\n\r\n",
            path, domain, ws_key
        );

        ws.stream.write_all(req.as_bytes()).await?;
        ws.stream.flush().await?;

        // Read response headers with 1KB chunks (far faster than 1-byte read calls)
        let mut response_buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        let end_idx = loop {
            let n = ws.stream.read(&mut chunk).await?;
            if n == 0 {
                return Err("Connection closed during WebSocket handshake".into());
            }
            response_buf.extend_from_slice(&chunk[..n]);

            if let Some(pos) = response_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }

            if response_buf.len() > 8192 {
                return Err("WebSocket handshake response too large".into());
            }
        };
        let resp_str = String::from_utf8_lossy(&response_buf[..end_idx]);
        if !resp_str.starts_with("HTTP/1.1 101") && !resp_str.starts_with("HTTP/1.0 101") {
            return Err(format!("WebSocket handshake failed: {}", resp_str.lines().next().unwrap_or("")).into());
        }

        let leftover = response_buf[end_idx + 4..].to_vec();
        ws.leftover = leftover;

        Ok(ws)
    }

    pub async fn send(&mut self, data: &[u8]) -> std::io::Result<()> {
        let frame = build_frame(OP_BINARY, data, true);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await
    }

    pub async fn close(&mut self) -> std::io::Result<()> {
        let frame = build_frame(OP_CLOSE, &[], true);
        let _ = self.stream.write_all(&frame).await;
        let _ = self.stream.flush().await;
        self.stream.shutdown().await
    }

    pub fn split(self) -> (WsReader, WsWriter) {
        let (reader, writer) = tokio::io::split(self.stream);
        (
            WsReader {
                reader,
                closed: false,
                frag: Vec::new(),
                leftover: self.leftover,
            },
            WsWriter {
                writer,
                closed: false,
            },
        )
    }
}

impl WsWriter {
    pub async fn send(&mut self, data: &[u8]) -> std::io::Result<()> {
        if self.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "WebSocket closed"));
        }
        let frame = build_frame(OP_BINARY, data, true);
        self.writer.write_all(&frame).await?;
        self.writer.flush().await
    }

    pub async fn send_batch(&mut self, parts: &[Vec<u8>]) -> std::io::Result<()> {
        if self.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "WebSocket closed"));
        }
        let total_size: usize = parts.iter().map(|p| p.len() + 14).sum();
        let mut batch = Vec::with_capacity(total_size);
        for part in parts {
            let frame = build_frame(OP_BINARY, part, true);
            batch.extend_from_slice(&frame);
        }
        self.writer.write_all(&batch).await?;
        self.writer.flush().await
    }

    pub async fn close(&mut self) -> std::io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let frame = build_frame(OP_CLOSE, &[], true);
        let _ = self.writer.write_all(&frame).await;
        let _ = self.writer.flush().await;
        self.writer.shutdown().await
    }
}

impl WsReader {
    pub async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        while !self.closed {
            let (opcode, payload, fin) = self.read_frame().await?;

            if opcode == OP_CLOSE {
                self.closed = true;
                return Ok(None);
            }

            if opcode == OP_PING || opcode == OP_PONG {
                continue;
            }

            if opcode == OP_CONT || opcode == OP_TEXT || opcode == OP_BINARY {
                if fin && self.frag.is_empty() {
                    return Ok(Some(payload));
                }
                self.frag.extend_from_slice(&payload);
                if self.frag.len() > MAX_MESSAGE_LEN {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "WebSocket message exceeds limit",
                    ));
                }
                if !fin {
                    continue;
                }
                let msg = std::mem::take(&mut self.frag);
                return Ok(Some(msg));
            }
        }
        Ok(None)
    }

    async fn read_exact_buffered(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        let mut needed = buf.len();
        let mut offset = 0;

        if !self.leftover.is_empty() {
            let take = self.leftover.len().min(needed);
            buf[..take].copy_from_slice(&self.leftover[..take]);
            self.leftover.drain(..take);
            needed -= take;
            offset += take;
        }

        if needed > 0 {
            self.reader.read_exact(&mut buf[offset..]).await?;
        }

        Ok(())
    }

    async fn read_frame(&mut self) -> std::io::Result<(u8, Vec<u8>, bool)> {
        let mut hdr = [0u8; 2];
        self.read_exact_buffered(&mut hdr).await?;

        let fin = (hdr[0] & 0x80) != 0;
        let opcode = hdr[0] & 0x0F;
        let masked = (hdr[1] & 0x80) != 0;
        let raw_len = hdr[1] & 0x7F;

        let length = if raw_len == 126 {
            let mut b = [0u8; 2];
            self.read_exact_buffered(&mut b).await?;
            u16::from_be_bytes(b) as usize
        } else if raw_len == 127 {
            let mut b = [0u8; 8];
            self.read_exact_buffered(&mut b).await?;
            u64::from_be_bytes(b) as usize
        } else {
            raw_len as usize
        };

        if length > MAX_MESSAGE_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WS frame too large",
            ));
        }

        let mask_key = if masked {
            let mut m = [0u8; 4];
            self.read_exact_buffered(&mut m).await?;
            Some(m)
        } else {
            None
        };

        let mut payload = vec![0u8; length];
        self.read_exact_buffered(&mut payload).await?;

        if let Some(mask) = mask_key {
            xor_mask(&mut payload, &mask);
        }

        Ok((opcode, payload, fin))
    }
}

fn build_frame(opcode: u8, data: &[u8], mask: bool) -> Vec<u8> {
    let length = data.len();
    let fb = 0x80 | opcode;
    let mut buf = Vec::with_capacity(14 + length);
    buf.push(fb);

    if !mask {
        if length < 126 {
            buf.push(length as u8);
        } else if length < 65536 {
            buf.push(126);
            buf.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            buf.push(127);
            buf.extend_from_slice(&(length as u64).to_be_bytes());
        }
        buf.extend_from_slice(data);
    } else {
        let mut mask_key = [0u8; 4];
        rand::rng().fill(&mut mask_key);

        if length < 126 {
            buf.push(0x80 | (length as u8));
        } else if length < 65536 {
            buf.push(0x80 | 126);
            buf.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            buf.push(0x80 | 127);
            buf.extend_from_slice(&(length as u64).to_be_bytes());
        }
        buf.extend_from_slice(&mask_key);

        let mut masked = data.to_vec();
        xor_mask(&mut masked, &mask_key);
        buf.extend_from_slice(&masked);
    }

    buf
}
