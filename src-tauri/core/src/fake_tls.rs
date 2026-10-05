use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

pub const TLS_RECORD_HANDSHAKE: u8 = 0x16;
pub const TLS_RECORD_CCS: u8 = 0x14;
pub const TLS_RECORD_APPDATA: u8 = 0x17;

pub const CLIENT_RANDOM_OFFSET: usize = 11;
pub const CLIENT_RANDOM_LEN: usize = 32;
pub const SESSION_ID_OFFSET: usize = 44;
pub const SESSION_ID_LEN: usize = 32;
pub const TIMESTAMP_TOLERANCE: i64 = 120;
pub const TLS_APPDATA_MAX: usize = 16384;

const CCS_FRAME: &[u8] = &[0x14, 0x03, 0x03, 0x00, 0x01, 0x01];

pub struct FakeTlsHandshake {
    pub client_random: [u8; 32],
    pub session_id: [u8; 32],
    pub timestamp: u32,
}

pub fn verify_client_hello(data: &[u8], secret: &[u8]) -> Option<FakeTlsHandshake> {
    let n = data.len();
    if n < 43 || data[0] != TLS_RECORD_HANDSHAKE || data[5] != 0x01 {
        return None;
    }

    let mut client_random = [0u8; 32];
    client_random.copy_from_slice(&data[CLIENT_RANDOM_OFFSET..CLIENT_RANDOM_OFFSET + 32]);

    let mut zeroed = data.to_vec();
    for b in &mut zeroed[CLIENT_RANDOM_OFFSET..CLIENT_RANDOM_OFFSET + 32] {
        *b = 0;
    }

    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(&zeroed);
    let expected = mac.finalize().into_bytes();

    if client_random[..28] != expected[..28] {
        return None;
    }

    let mut ts_bytes = [0u8; 4];
    for i in 0..4 {
        ts_bytes[i] = client_random[28 + i] ^ expected[28 + i];
    }
    let timestamp = u32::from_le_bytes(ts_bytes);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;

    if (now - (timestamp as i64)).abs() > TIMESTAMP_TOLERANCE {
        return None;
    }

    let mut session_id = [0u8; 32];
    if n >= SESSION_ID_OFFSET + SESSION_ID_LEN && data[43] == 0x20 {
        session_id.copy_from_slice(&data[SESSION_ID_OFFSET..SESSION_ID_OFFSET + 32]);
    }

    Some(FakeTlsHandshake {
        client_random,
        session_id,
        timestamp,
    })
}

pub fn build_server_hello(secret: &[u8], client_random: &[u8; 32], session_id: &[u8; 32]) -> Vec<u8> {
    let mut sh = vec![
        0x16, 0x03, 0x03, 0x00, 0x7a,
        0x02, 0x00, 0x00, 0x76,
        0x03, 0x03,
    ];
    // Random placeholder (32 bytes)
    sh.extend_from_slice(&[0u8; 32]);
    // Session ID len (0x20)
    sh.push(0x20);
    // Session ID (32 bytes)
    sh.extend_from_slice(session_id);
    // Cipher suite TLS_AES_128_GCM_SHA256 (0x13, 0x01) + compression (0x00)
    sh.extend_from_slice(&[0x13, 0x01, 0x00]);
    // Extensions
    sh.extend_from_slice(&[
        0x00, 0x2e,
        0x00, 0x33, 0x00, 0x24, 0x00, 0x1d, 0x00, 0x20,
    ]);
    // Key share (32 random bytes)
    let mut pubkey = [0u8; 32];
    rand::RngExt::fill(&mut rand::rng(), &mut pubkey);
    sh.extend_from_slice(&pubkey);
    // Supported versions (TLS 1.3)
    sh.extend_from_slice(&[0x00, 0x2b, 0x00, 0x02, 0x03, 0x04]);

    let mut rng = rand::rng();
    let enc_size: usize = rng.random_range(1900..=2100);
    let mut enc_data = vec![0u8; enc_size];
    rng.fill(&mut enc_data);

    let mut app_record = vec![0x17, 0x03, 0x03];
    app_record.extend_from_slice(&(enc_size as u16).to_be_bytes());
    app_record.extend_from_slice(&enc_data);

    let mut response = Vec::new();
    response.extend_from_slice(&sh);
    response.extend_from_slice(CCS_FRAME);
    response.extend_from_slice(&app_record);

    let mut hmac = HmacSha256::new_from_slice(secret).unwrap();
    hmac.update(client_random);
    hmac.update(&response);
    let server_random = hmac.finalize().into_bytes();

    // Patch server random at offset 11
    response[11..43].copy_from_slice(&server_random);

    response
}

pub fn wrap_tls_records(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(TLS_APPDATA_MAX) {
        out.push(0x17);
        out.push(0x03);
        out.push(0x03);
        out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct FakeTlsReader<R> {
    reader: R,
    buf: Vec<u8>,
    cursor: usize,
}

impl<R: AsyncReadExt + Unpin> FakeTlsReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::new(),
            cursor: 0,
        }
    }

    pub async fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.cursor >= self.buf.len() {
            self.buf.clear();
            self.cursor = 0;
            let mut hdr = [0u8; 5];
            match self.reader.read_exact(&mut hdr).await {
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) => return Err(e),
            }
            let rtype = hdr[0];
            let rec_len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
            if rtype == TLS_RECORD_CCS {
                let mut sink = vec![0u8; rec_len];
                self.reader.read_exact(&mut sink).await?;
                continue;
            }
            if rtype != TLS_RECORD_APPDATA {
                return Ok(0);
            }
            self.buf.resize(rec_len, 0);
            self.reader.read_exact(&mut self.buf).await?;
        }
        let available = self.buf.len() - self.cursor;
        let to_copy = available.min(out.len());
        out[..to_copy].copy_from_slice(&self.buf[self.cursor..self.cursor + to_copy]);
        self.cursor += to_copy;
        Ok(to_copy)
    }

    pub async fn read_exact(&mut self, mut out: &mut [u8]) -> std::io::Result<()> {
        while !out.is_empty() {
            let n = self.read(out).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ));
            }
            out = &mut out[n..];
        }
        Ok(())
    }
}

pub struct FakeTlsWriter<W> {
    writer: W,
}

impl<W: AsyncWriteExt + Unpin> FakeTlsWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub async fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        let wrapped = wrap_tls_records(data);
        self.writer.write_all(&wrapped).await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.writer.shutdown().await
    }
}

pub enum ClientReader {
    Plain(tokio::net::tcp::OwnedReadHalf),
    FakeTls(FakeTlsReader<tokio::net::tcp::OwnedReadHalf>),
}

impl ClientReader {
    pub async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(r) => r.read(buf).await,
            Self::FakeTls(r) => r.read(buf).await,
        }
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(r) => r.read_exact(buf).await.map(|_| ()),
            Self::FakeTls(r) => r.read_exact(buf).await,
        }
    }
}

pub enum ClientWriter {
    Plain(tokio::net::tcp::OwnedWriteHalf),
    FakeTls(FakeTlsWriter<tokio::net::tcp::OwnedWriteHalf>),
}

impl ClientWriter {
    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => {
                w.write_all(buf).await?;
                w.flush().await?;
                Ok(())
            }
            Self::FakeTls(w) => w.write_all(buf).await,
        }
    }

    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => w.shutdown().await,
            Self::FakeTls(w) => w.shutdown().await,
        }
    }
}

pub async fn proxy_to_masking_domain(
    client_stream: TcpStream,
    initial_data: &[u8],
    domain: &str,
    label: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let target = format!("{}:443", domain);
    tracing::info!("[{}] Proxying probe to masking domain: {}", label, target);

    let upstream = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(&target),
    ).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!("[{}] Failed to connect to masking domain {}: {:?}", label, target, e);
            return Err(e.into());
        }
        Err(_) => {
            tracing::warn!("[{}] Timeout connecting to masking domain {}", label, target);
            return Err("Masking domain connect timeout".into());
        }
    };

    let mut upstream = upstream;
    let _ = upstream.set_nodelay(true);

    if !initial_data.is_empty() {
        upstream.write_all(initial_data).await?;
        upstream.flush().await?;
    }

    let (mut cr, mut cw) = client_stream.into_split();
    let (mut ur, mut uw) = upstream.into_split();

    let c_to_u = async move {
        let _ = tokio::io::copy(&mut cr, &mut uw).await;
        let _ = uw.shutdown().await;
    };
    let u_to_c = async move {
        let _ = tokio::io::copy(&mut ur, &mut cw).await;
        let _ = cw.shutdown().await;
    };

    tokio::select! {
        _ = c_to_u => {},
        _ = u_to_c => {},
    }

    Ok(())
}

