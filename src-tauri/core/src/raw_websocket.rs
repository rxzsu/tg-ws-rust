use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use rand::RngExt;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha1::{Digest as _, Sha1};

/// RFC 6455 handshake GUID.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

pub const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

/// Shared TLS client config, built once and reused by every upstream
/// connection. Sharing matters: rustls keeps the session-ticket store
/// inside `ClientConfig`, so a fresh config per connect (as before) meant
/// a full TLS handshake every time and re-cloned ~150 root certs.
/// With a shared config, repeat connects to the same DC can resume
/// sessions (1-RTT / abbreviated handshake).
static TLS_CONFIG: std::sync::OnceLock<Arc<ClientConfig>> = std::sync::OnceLock::new();

fn tls_config() -> Arc<ClientConfig> {
    TLS_CONFIG
        .get_or_init(|| {
            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

pub enum WsStream {
    Plain(TcpStream),
    /// Boxed: the TLS stream is ~1 KB, boxing keeps pool moves pointer-sized.
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
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

/// Read exactly `buf.len()` bytes, consuming any leftovers buffered after the
/// HTTP upgrade first.
async fn read_exact_buffered<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    leftover: &mut Vec<u8>,
    buf: &mut [u8],
) -> io::Result<()> {
    let mut needed = buf.len();
    let mut offset = 0;

    if !leftover.is_empty() {
        let take = leftover.len().min(needed);
        buf[..take].copy_from_slice(&leftover[..take]);
        leftover.drain(..take);
        needed -= take;
        offset += take;
    }

    if needed > 0 {
        reader.read_exact(&mut buf[offset..]).await?;
    }

    Ok(())
}

/// Read one WebSocket frame into `payload` (allocation reused by the caller).
async fn read_frame_into<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    leftover: &mut Vec<u8>,
    payload: &mut Vec<u8>,
) -> io::Result<(u8, bool)> {
    let mut hdr = [0u8; 2];
    read_exact_buffered(reader, leftover, &mut hdr).await?;

    let fin = (hdr[0] & 0x80) != 0;
    let opcode = hdr[0] & 0x0F;
    let masked = (hdr[1] & 0x80) != 0;
    let raw_len = hdr[1] & 0x7F;

    let length = if raw_len == 126 {
        let mut b = [0u8; 2];
        read_exact_buffered(reader, leftover, &mut b).await?;
        u16::from_be_bytes(b) as usize
    } else if raw_len == 127 {
        let mut b = [0u8; 8];
        read_exact_buffered(reader, leftover, &mut b).await?;
        u64::from_be_bytes(b) as usize
    } else {
        raw_len as usize
    };

    if length > MAX_MESSAGE_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebSocket frame too large",
        ));
    }

    let mask_key = if masked {
        let mut m = [0u8; 4];
        read_exact_buffered(reader, leftover, &mut m).await?;
        Some(m)
    } else {
        None
    };

    // Reuse the caller's allocation: `resize` only grows when the frame
    // is larger than the current capacity.
    payload.clear();
    payload.resize(length, 0);
    read_exact_buffered(reader, leftover, payload).await?;

    if let Some(mask) = mask_key {
        xor_mask(payload, &mask);
    }

    Ok((opcode, fin))
}

/// Case-insensitive HTTP header lookup.
fn header_value<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
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
    /// Scratch buffer for `send_batch`, reused across calls.
    batch: Vec<u8>,
}

impl RawWebSocket {
    pub async fn connect(
        host: &str,
        domain: &str,
        path: &str,
        secure: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::connect_with_sni(host, domain, path, secure, None, crate::net::clamp_io_buf(262144)).await
    }

    pub async fn connect_with_buf(
        host: &str,
        domain: &str,
        path: &str,
        secure: bool,
        io_buf: usize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::connect_with_sni(host, domain, path, secure, None, io_buf).await
    }

    pub async fn connect_with_sni(
        host: &str,
        domain: &str,
        path: &str,
        secure: bool,
        sni: Option<&str>,
        io_buf: usize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let stream = if secure {
            let tcp = crate::net::tcp_connect(host, 443, io_buf).await?;
            let _ = tcp.set_nodelay(true);

            let tls_sni = sni.unwrap_or(domain);
            let server_name = ServerName::try_from(tls_sni.to_string())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

            // ECH: hide the real SNI from DPI. If the server rejects the
            // encrypted handshake (rotated ECH key, non-ECH edge) fall back
            // to one plain-TLS attempt — degraded latency, not connectivity.
            if let Some(ech_cfg) = crate::ech::tls_config_for(tls_sni).await {
                let ech_connector = TlsConnector::from(ech_cfg);
                match ech_connector.connect(server_name.clone(), tcp).await {
                    Ok(tls) => {
                        tracing::info!("ECH handshake OK for {}", tls_sni);
                        WsStream::Tls(Box::new(tls))
                    }
                    Err(e) => {
                        tracing::warn!("ECH handshake to {} failed ({}), retrying without ECH", tls_sni, e);
                        let tcp = crate::net::tcp_connect(host, 443, io_buf).await?;
                        let _ = tcp.set_nodelay(true);
                        let connector = TlsConnector::from(tls_config());
                        let tls = connector.connect(server_name, tcp).await?;
                        WsStream::Tls(Box::new(tls))
                    }
                }
            } else {
                let connector = TlsConnector::from(tls_config());
                let tls = connector.connect(server_name, tcp).await?;
                WsStream::Tls(Box::new(tls))
            }
        } else {
            let tcp = crate::net::tcp_connect(host, 80, io_buf).await?;
            let _ = tcp.set_nodelay(true);
            WsStream::Plain(tcp)
        };

        let mut ws = Self { stream, leftover: Vec::new() };
        Self::complete_ws_upgrade(&mut ws, domain, path).await?;
        Ok(ws)
    }

    /// Send the HTTP/1.1 Upgrade request and validate the 101 response
    /// (split out so the ECH path reuses it unchanged).
    async fn complete_ws_upgrade(
        &mut self,
        domain: &str,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

        self.stream.write_all(req.as_bytes()).await?;
        self.stream.flush().await?;

        // Read response headers with 1KB chunks (far faster than 1-byte read calls)
        let mut response_buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        let end_idx = loop {
            let n = self.stream.read(&mut chunk).await?;
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

        // RFC 6455: verify the server really completed the upgrade instead of
        // trusting the status line alone.
        if !header_value(&resp_str, "upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        {
            return Err("WebSocket handshake failed: missing `Upgrade: websocket`".into());
        }
        let mut accept_hasher = Sha1::new();
        accept_hasher.update(ws_key.as_bytes());
        accept_hasher.update(WS_GUID.as_bytes());
        let expected_accept = BASE64.encode(accept_hasher.finalize());
        match header_value(&resp_str, "sec-websocket-accept") {
            Some(value) if value == expected_accept => {}
            Some(value) => {
                return Err(format!(
                    "WebSocket handshake failed: Sec-WebSocket-Accept mismatch (got {}, expected {})",
                    value, expected_accept
                )
                .into());
            }
            None => {
                return Err("WebSocket handshake failed: missing Sec-WebSocket-Accept".into());
            }
        }

        let leftover = response_buf[end_idx + 4..].to_vec();
        self.leftover = leftover;

        Ok(())
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

    /// Keep-alive ping for pooled sockets.
    ///
    /// Sends a real WS ping and waits for the pong, so a socket that still
    /// accepts writes into the kernel buffer but is already dead upstream is
    /// detected and dropped. Writes and reads go through TLS too.
    pub async fn ping_pong(&mut self) -> io::Result<()> {
        self.stream.write_all(&build_frame(OP_PING, b"ka", true)).await?;
        self.stream.flush().await?;
        let mut payload = Vec::new();
        loop {
            let (opcode, _) = self.read_frame(&mut payload).await?;
            match opcode {
                OP_PONG => return Ok(()),
                OP_PING => {
                    // Stay a well-behaved WS peer (rare for a bare upstream).
                    let pong = build_frame(OP_PONG, &payload, true);
                    self.stream.write_all(&pong).await?;
                    self.stream.flush().await?;
                }
                OP_CLOSE => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "WebSocket closed during keep-alive",
                    ));
                }
                _ => {}
            }
        }
    }

    async fn read_frame(&mut self, payload: &mut Vec<u8>) -> io::Result<(u8, bool)> {
        read_frame_into(&mut self.stream, &mut self.leftover, payload).await
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
                batch: Vec::new(),
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

    pub async fn send_batch(&mut self, parts: &[&[u8]]) -> std::io::Result<()> {
        if self.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "WebSocket closed"));
        }
        // One reusable buffer per writer instead of a fresh Vec per batch.
        let mut batch = std::mem::take(&mut self.batch);
        batch.clear();
        let total_size: usize = parts.iter().map(|p| p.len() + 14).sum();
        batch.reserve(total_size);
        for part in parts {
            encode_frame(&mut batch, OP_BINARY, part, true);
        }
        let res = match self.writer.write_all(&batch).await {
            Ok(()) => self.writer.flush().await,
            Err(e) => Err(e),
        };
        // Keep the allocation for the next batch, even on write errors.
        self.batch = batch;
        res
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
        let mut buf = Vec::new();
        if self.recv_into(&mut buf).await? {
            Ok(Some(buf))
        } else {
            Ok(None)
        }
    }

    /// Receive the next message into `out`, reusing its allocation.
    ///
    /// The bridge keeps one buffer per session, so media streaming does not
    /// allocate a fresh `Vec` for every frame. Returns `Ok(false)` when the
    /// peer closed the stream.
    pub async fn recv_into(&mut self, out: &mut Vec<u8>) -> std::io::Result<bool> {
        while !self.closed {
            out.clear();
            let (opcode, fin) = self.read_frame_into(out).await?;

            if opcode == OP_CLOSE {
                self.closed = true;
                return Ok(false);
            }

            if opcode == OP_PING || opcode == OP_PONG {
                continue;
            }

            if opcode == OP_CONT || opcode == OP_TEXT || opcode == OP_BINARY {
                if fin && self.frag.is_empty() {
                    return Ok(true);
                }
                self.frag.extend_from_slice(out);
                if self.frag.len() > MAX_MESSAGE_LEN {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "WebSocket message exceeds limit",
                    ));
                }
                if !fin {
                    continue;
                }
                // Hand the assembled message to the caller and keep the
                // scratch allocation for the next fragmented frame.
                std::mem::swap(&mut self.frag, out);
                self.frag.clear();
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn read_frame_into(&mut self, payload: &mut Vec<u8>) -> std::io::Result<(u8, bool)> {
        read_frame_into(&mut self.reader, &mut self.leftover, payload).await
    }
}

fn build_frame(opcode: u8, data: &[u8], mask: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(14 + data.len());
    encode_frame(&mut buf, opcode, data, mask);
    buf
}

/// Append one frame to `out` in place (no intermediate `Vec`, no extra copy
/// beyond the mandatory masking pass).
fn encode_frame(out: &mut Vec<u8>, opcode: u8, data: &[u8], mask: bool) {
    let length = data.len();
    out.push(0x80 | opcode);

    let mask_bit = if mask { 0x80 } else { 0 };
    if length < 126 {
        out.push(mask_bit | (length as u8));
    } else if length < 65536 {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(length as u64).to_be_bytes());
    }

    if !mask {
        out.extend_from_slice(data);
        return;
    }

    let mut mask_key = [0u8; 4];
    rand::rng().fill(&mut mask_key);
    out.extend_from_slice(&mask_key);

    let start = out.len();
    out.extend_from_slice(data);
    xor_mask(&mut out[start..], &mask_key);
}
