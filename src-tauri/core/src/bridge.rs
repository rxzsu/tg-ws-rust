use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use aes::cipher::StreamCipher;
use crate::bufpool::acquire_buf;
use crate::crypto::CryptoCtx;
use crate::fake_tls::{ClientReader, ClientWriter};
use crate::raw_websocket::RawWebSocket;
use crate::splitter::MsgSplitter;
use crate::stats::Stats;
use tracing::{debug, info};

/// Millis of the last observed session activity (epoch). Updated on every
/// read in both directions; the session watchdog reaps the connection when
/// it stops moving.
pub type ActivityStamp = Arc<std::sync::atomic::AtomicU64>;

fn touch(activity: &ActivityStamp) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    activity.store(now, std::sync::atomic::Ordering::Relaxed);
}

/// Optional per-bridge tuning knobs (keeps the bridge signatures small).
pub struct BridgeOptions {
    pub splitter: Option<MsgSplitter>,
    pub io_buf: usize,
}

/// Upstream halves for the fallback bridge: plain TCP after `into_split`,
/// or a QUIC bidirectional stream pair. Anything `AsyncRead`/`AsyncWrite`.
pub struct RemoteIo<R, W> {
    pub reader: R,
    pub writer: W,
}

#[allow(clippy::too_many_arguments)]
pub async fn bridge_ws_reencrypt(
    mut client_reader: ClientReader,
    mut client_writer: ClientWriter,
    ws: RawWebSocket,
    ctx: CryptoCtx,
    opts: BridgeOptions,
    stats: Arc<Stats>,
    label: String,
    activity: ActivityStamp,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut ws_reader, mut ws_writer, ws_ponger) = ws.split();
    // Single allocation sized from config instead of a hardcoded 64 KB.
    let buf_size = crate::net::clamp_io_buf(opts.io_buf);
    let mut splitter = opts.splitter;

    let mut clt_dec = ctx.clt_dec;
    let mut clt_enc = ctx.clt_enc;
    let mut tg_enc = ctx.tg_enc;
    let mut tg_dec = ctx.tg_dec;

    let stats_up = stats.clone();
    let stats_down = stats.clone();
    let activity_up = activity.clone();
    let activity_down = activity.clone();
    let label_up = label.clone();
    let label_down = label.clone();

    // Direct Upload Pipeline: Client -> Decrypt & Re-encrypt -> WS
    let upload_task = async move {
        let mut buf = acquire_buf(buf_size);
        loop {
            let n = match client_reader.read(&mut buf[..]).await {
                Ok(0) => break,
                Ok(n) => {
                    touch(&activity_up);
                    n
                }
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
                if !parts.is_empty()
                    && let Err(e) = ws_writer.send_batch(&parts).await
                {
                    debug!("[{}] ws send_batch error: {:?}", label_up, e);
                    break;
                }
            } else {
                if let Err(e) = ws_writer.send(&buf[..n]).await {
                    debug!("[{}] ws send error: {:?}", label_up, e);
                    break;
                }
            }
        }

        if let Some(ref mut sp) = splitter
            && let Some(tail) = sp.flush()
        {
            let _ = ws_writer.send(&tail).await;
        }
        let _ = ws_writer.close().await;
    };

    // Direct Download Pipeline: WS -> Decrypt & Re-encrypt -> Client
    let download_task = async move {
        // One pooled buffer for the whole session: `recv_into` reuses its
        // allocation instead of allocating a fresh Vec per WS frame.
        let mut msg = acquire_buf(buf_size);
        loop {
            match ws_reader.recv_into(&mut msg).await {
                Ok(true) => {
                    // Upstream PING: answer it and read on. Not answered,
                    // CDN fronts tear the connection after ~100 s of
                    // silence, killing every idle-but-healthy session.
                    if let Some(pong_payload) = ws_reader.take_pending_pong() {
                        if let Err(e) = ws_ponger.send_pong(&pong_payload).await {
                            debug!("[{}] ws pong error: {:?}", label_down, e);
                            break;
                        }
                        continue;
                    }

                    touch(&activity_down);
                    let n = msg.len();
                    stats_down.add_down(n);

                    // In-place decryption and re-encryption
                    tg_dec.apply_keystream(&mut msg[..]);
                    clt_enc.apply_keystream(&mut msg[..]);

                    if let Err(e) = client_writer.write_all(&msg[..]).await {
                        debug!("[{}] client write error: {:?}", label_down, e);
                        break;
                    }
                }
                Ok(false) => break,
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

#[allow(clippy::too_many_arguments)]
pub async fn bridge_tcp_fallback<R, W>(
    mut client_reader: ClientReader,
    mut client_writer: ClientWriter,
    remote: RemoteIo<R, W>,
    ctx: CryptoCtx,
    stats: Arc<Stats>,
    label: String,
    io_buf: usize,
    activity: ActivityStamp,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: tokio::io::AsyncRead + Unpin + Send,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    let buf_size = crate::net::clamp_io_buf(io_buf);
    let mut remote_reader = remote.reader;
    let mut remote_writer = remote.writer;

    let mut clt_dec = ctx.clt_dec;
    let mut clt_enc = ctx.clt_enc;
    let mut tg_enc = ctx.tg_enc;
    let mut tg_dec = ctx.tg_dec;

    let stats_up = stats.clone();
    let stats_down = stats.clone();
    let activity_up = activity.clone();
    let activity_down = activity.clone();
    let label_up = label.clone();
    let label_down = label.clone();

    // Client -> Decrypt (clt_dec) -> Encrypt (tg_enc) -> Remote TCP
    let upload_task = async move {
        let mut buf = acquire_buf(buf_size);
        loop {
            let n = match client_reader.read(&mut buf[..]).await {
                Ok(0) => break,
                Ok(n) => {
                    touch(&activity_up);
                    n
                }
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
        let mut buf = acquire_buf(buf_size);
        loop {
            let n = match remote_reader.read(&mut buf[..]).await {
                Ok(0) => break,
                Ok(n) => {
                    touch(&activity_down);
                    n
                }
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

