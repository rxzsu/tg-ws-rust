use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use aes::cipher::StreamCipher;
use crate::crypto::CryptoCtx;
use crate::fake_tls::{ClientReader, ClientWriter};
use crate::raw_websocket::RawWebSocket;
use crate::splitter::MsgSplitter;
use crate::stats::Stats;
use tracing::{debug, info};

pub async fn bridge_ws_reencrypt(
    mut client_reader: ClientReader,
    mut client_writer: ClientWriter,
    ws: RawWebSocket,
    ctx: CryptoCtx,
    mut splitter: Option<MsgSplitter>,
    stats: Arc<Stats>,
    label: String,
    io_buf: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut ws_reader, mut ws_writer) = ws.split();
    // Single allocation sized from config instead of a hardcoded 64 KB.
    let buf_size = crate::net::clamp_io_buf(io_buf);

    let mut clt_dec = ctx.clt_dec;
    let mut clt_enc = ctx.clt_enc;
    let mut tg_enc = ctx.tg_enc;
    let mut tg_dec = ctx.tg_dec;

    let stats_up = stats.clone();
    let stats_down = stats.clone();
    let label_up = label.clone();
    let label_down = label.clone();

    // Direct Upload Pipeline: Client -> Decrypt & Re-encrypt -> WS
    let upload_task = async move {
        let mut buf = vec![0u8; buf_size];
        loop {
            let n = match client_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    debug!("[{}] client read error: {:?}", label_up, e);
                    break;
                }
            };

            stats_up.add_up(n);

            // In-place zero-copy decryption and re-encryption
            clt_dec.apply_keystream(&mut buf[..n]);
            tg_enc.apply_keystream(&mut buf[..n]);

            if let Some(ref mut sp) = splitter {
                let parts = sp.split(&buf[..n]);
                if !parts.is_empty() {
                    if let Err(e) = ws_writer.send_batch(&parts).await {
                        debug!("[{}] ws send_batch error: {:?}", label_up, e);
                        break;
                    }
                }
            } else {
                if let Err(e) = ws_writer.send(&buf[..n]).await {
                    debug!("[{}] ws send error: {:?}", label_up, e);
                    break;
                }
            }
        }

        if let Some(ref mut sp) = splitter {
            let flushed = sp.flush();
            if !flushed.is_empty() {
                let _ = ws_writer.send_batch(&flushed).await;
            }
        }
        let _ = ws_writer.close().await;
    };

    // Direct Download Pipeline: WS -> Decrypt & Re-encrypt -> Client
    let download_task = async move {
        loop {
            match ws_reader.recv().await {
                Ok(Some(mut msg)) => {
                    let n = msg.len();
                    stats_down.add_down(n);

                    // In-place decryption and re-encryption
                    tg_dec.apply_keystream(&mut msg);
                    clt_enc.apply_keystream(&mut msg);

                    if let Err(e) = client_writer.write_all(&msg).await {
                        debug!("[{}] client write error: {:?}", label_down, e);
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    debug!("[{}] ws read error: {:?}", label_down, e);
                    break;
                }
            }
        }
        let _ = client_writer.shutdown().await;
    };

    tokio::select! {
        _ = upload_task => {},
        _ = download_task => {},
    }

    info!("[{}] bridge session finished", label);
    Ok(())
}

pub async fn bridge_tcp_fallback(
    mut client_reader: ClientReader,
    mut client_writer: ClientWriter,
    remote_stream: TcpStream,
    ctx: CryptoCtx,
    stats: Arc<Stats>,
    label: String,
    io_buf: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut remote_reader, mut remote_writer) = remote_stream.into_split();
    let buf_size = crate::net::clamp_io_buf(io_buf);

    let mut clt_dec = ctx.clt_dec;
    let mut clt_enc = ctx.clt_enc;
    let mut tg_enc = ctx.tg_enc;
    let mut tg_dec = ctx.tg_dec;

    let stats_up = stats.clone();
    let stats_down = stats.clone();
    let label_up = label.clone();
    let label_down = label.clone();

    // Client -> Decrypt (clt_dec) -> Encrypt (tg_enc) -> Remote TCP
    let upload_task = async move {
        let mut buf = vec![0u8; buf_size];
        loop {
            let n = match client_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    debug!("[{}] client read error in TCP fallback: {:?}", label_up, e);
                    break;
                }
            };
            stats_up.add_up(n);
            clt_dec.apply_keystream(&mut buf[..n]);
            tg_enc.apply_keystream(&mut buf[..n]);
            if let Err(e) = remote_writer.write_all(&buf[..n]).await {
                debug!("[{}] remote TCP write error: {:?}", label_up, e);
                break;
            }
        }
        let _ = remote_writer.shutdown().await;
    };

    // Remote TCP -> Decrypt (tg_dec) -> Encrypt (clt_enc) -> Client
    let download_task = async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; buf_size];
        loop {
            let n = match remote_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    debug!("[{}] remote TCP read error in TCP fallback: {:?}", label_down, e);
                    break;
                }
            };
            stats_down.add_down(n);
            tg_dec.apply_keystream(&mut buf[..n]);
            clt_enc.apply_keystream(&mut buf[..n]);
            if let Err(e) = client_writer.write_all(&buf[..n]).await {
                debug!("[{}] client write error in TCP fallback: {:?}", label_down, e);
                break;
            }
        }
        let _ = client_writer.shutdown().await;
    };

    tokio::select! {
        _ = upload_task => {},
        _ = download_task => {},
    }

    info!("[{}] Direct TCP fallback session finished", label);
    Ok(())
}

