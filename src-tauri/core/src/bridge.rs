use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use aes::cipher::StreamCipher;
use crate::crypto::CryptoCtx;
use crate::raw_websocket::RawWebSocket;
use crate::splitter::MsgSplitter;
use crate::stats::Stats;
use tracing::{debug, info};

pub async fn bridge_ws_reencrypt(
    client_stream: TcpStream,
    ws: RawWebSocket,
    ctx: CryptoCtx,
    mut splitter: Option<MsgSplitter>,
    stats: Arc<Stats>,
    label: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut client_reader, mut client_writer) = tokio::io::split(client_stream);
    let (mut ws_reader, mut ws_writer) = ws.split();

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
        let mut buf = vec![0u8; 65536];
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
