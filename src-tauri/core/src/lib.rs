pub mod bridge;
pub mod config;
pub mod crypto;
pub mod fake_tls;
pub mod handshake;
pub mod pool;
pub mod raw_websocket;
pub mod splitter;
pub mod stats;

use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info, warn};

use crate::bridge::bridge_ws_reencrypt;
use crate::config::{get_default_dc_ip, ProxyConfig};
use crate::crypto::CryptoCtx;
use crate::fake_tls::{build_server_hello, verify_client_hello, TLS_RECORD_HANDSHAKE};
use crate::handshake::{generate_relay_init, try_handshake};
use crate::pool::ConnectionPool;
use crate::raw_websocket::RawWebSocket;
use crate::splitter::MsgSplitter;
pub use crate::stats::{Stats, TelemetrySnapshot};

pub async fn run_server_with_stats(
    config: ProxyConfig,
    stats: Arc<Stats>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&addr).await?;
    info!("tg-ws-proxy listening on {}", addr);

    let secret_bytes = hex::decode(&config.secret).unwrap_or_else(|_| vec![0u8; 16]);
    let secret = Arc::new(secret_bytes);
    let cfg = Arc::new(config);
    let pool = Arc::new(ConnectionPool::new(cfg.pool_size.max(1)));
    if cfg.pool_size > 0 {
        pool.warm_up_defaults(!cfg.disable_secure).await;
    }

    loop {
        let (stream, peer_addr) = tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Accept error: {:?}", e);
                        continue;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Server shutting down gracefully");
                return Ok(());
            }
        };

        let secret = secret.clone();
        let stats = stats.clone();
        let cfg = cfg.clone();
        let pool = pool.clone();
        let mut conn_shutdown = shutdown_rx.resubscribe();

        tokio::spawn(async move {
            stats.inc_total();
            stats.inc_active();
            let label = peer_addr.to_string();

            tokio::select! {
                res = handle_connection(stream, secret, stats.clone(), cfg, pool, label.clone()) => {
                    if let Err(e) = res {
                        warn!("[{}] connection error: {:?}", label, e);
                    }
                }
                _ = conn_shutdown.recv() => {
                    info!("[{}] connection closed due to proxy stop", label);
                }
            }
            stats.dec_active();
        });
    }
}

pub async fn run_server(config: ProxyConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stats = Arc::new(Stats::new());
    let (_tx, rx) = tokio::sync::broadcast::channel(1);
    run_server_with_stats(config, stats, rx).await
}

async fn handle_connection(
    mut stream: TcpStream,
    secret: Arc<Vec<u8>>,
    stats: Arc<Stats>,
    cfg: Arc<ProxyConfig>,
    pool: Arc<ConnectionPool>,
    label: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = stream.set_nodelay(true);

    let mut first_byte = [0u8; 1];
    stream.read_exact(&mut first_byte).await?;

    let handshake_buf = if first_byte[0] == TLS_RECORD_HANDSHAKE && !cfg.fake_tls_domain.is_empty() {
        // Fake TLS client hello
        let mut rest_hdr = [0u8; 4];
        stream.read_exact(&mut rest_hdr).await?;
        let record_len = u16::from_be_bytes([rest_hdr[2], rest_hdr[3]]) as usize;
        let mut record_body = vec![0u8; record_len];
        stream.read_exact(&mut record_body).await?;

        let mut client_hello = Vec::new();
        client_hello.push(first_byte[0]);
        client_hello.extend_from_slice(&rest_hdr);
        client_hello.extend_from_slice(&record_body);

        let tls_res = match verify_client_hello(&client_hello, &secret) {
            Some(res) => res,
            None => return Err("Fake TLS verification failed".into()),
        };

        let sh = build_server_hello(&secret, &tls_res.client_random, &tls_res.session_id);
        tokio::io::AsyncWriteExt::write_all(&mut stream, &sh).await?;

        // Read inner obfs2 handshake from TLS stream
        let mut inner_buf = [0u8; 64];
        let mut rec_hdr = [0u8; 5];
        stream.read_exact(&mut rec_hdr).await?;
        stream.read_exact(&mut inner_buf).await?;
        inner_buf
    } else {
        let mut rest = [0u8; 63];
        stream.read_exact(&mut rest).await?;
        let mut full = [0u8; 64];
        full[0] = first_byte[0];
        full[1..].copy_from_slice(&rest);
        full
    };

    let hs = match try_handshake(&handshake_buf, &secret) {
        Some(res) => res,
        None => {
            stats.connections_bad.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err("Invalid handshake".into());
        }
    };

    info!(
        "[{}] Handshake OK: DC{} (media={}) proto=0x{:08X}",
        label, hs.dc_id, hs.is_media, hs.proto_int
    );

    let mut dc_id = hs.dc_id;
    let is_test = cfg.force_test_dc || dc_id >= 10000;
    if dc_id >= 10000 {
        dc_id -= 10000;
    }

    let dc_idx = if hs.is_media { -(dc_id as i16) } else { dc_id as i16 };
    let relay_init = generate_relay_init(&hs.proto_tag, dc_idx);
    let ctx = CryptoCtx::new(&hs.dec_prekey_and_iv, &secret, &relay_init);

    // 1. Try WebSocket Domains with connection pooling & background replenishment
    let domains = crate::config::ws_domains(dc_id, hs.is_media);
    let mut ws: Option<RawWebSocket> = None;

    for domain in &domains {
        // Pool check
        if let Some(mut pooled) = pool.get(domain).await {
            // Auto-replenish pool in background so next client connection also gets 0ms latency
            let pool_bg = pool.clone();
            let domain_bg = domain.to_string();
            let secure_bg = !cfg.disable_secure;
            tokio::spawn(async move {
                if let Ok(new_ws) = RawWebSocket::connect(&domain_bg, &domain_bg, "/apiws", secure_bg).await {
                    pool_bg.put(domain_bg, new_ws).await;
                }
            });

            if pooled.send(&relay_init).await.is_ok() {
                stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ws = Some(pooled);
                break;
            }
        }

        info!("[{}] Connecting to upstream WS {}", label, domain);
        match RawWebSocket::connect(domain, domain, "/apiws", !cfg.disable_secure).await {
            Ok(mut socket) => {
                if let Err(e) = socket.send(&relay_init).await {
                    warn!("[{}] Failed to send relay init to {}: {:?}", label, domain, e);
                    continue;
                }
                stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ws = Some(socket);
                break;
            }
            Err(e) => {
                warn!("[{}] Connection to {} failed: {:?}", label, domain, e);
            }
        }
    }

    // 2. Fallback: CF Worker / CF Domains if direct WS failed
    if ws.is_none() && cfg.fallback_cfproxy {
        let fallback_ip = get_default_dc_ip(dc_id, is_test);

        // Try CF Worker domains
        if let Some(ip) = fallback_ip {
            for worker_domain in &cfg.cfproxy_worker_domains {
                let path = format!("/apiws?dst={}&dc={}", ip, hs.dc_id);
                info!("[{}] Fallback CF Worker: {} -> {}", label, worker_domain, path);
                if let Ok(mut worker_ws) = RawWebSocket::connect(worker_domain, worker_domain, &path, !cfg.disable_secure).await {
                    if worker_ws.send(&relay_init).await.is_ok() {
                        stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        ws = Some(worker_ws);
                        break;
                    }
                }
            }
        }

        // Try User CF domains
        if ws.is_none() {
            for cf_domain in &cfg.cfproxy_user_domains {
                info!("[{}] Fallback User CF Domain: {}", label, cf_domain);
                if let Ok(mut cf_ws) = RawWebSocket::connect(cf_domain, cf_domain, "/apiws", !cfg.disable_secure).await {
                    if cf_ws.send(&relay_init).await.is_ok() {
                        stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        ws = Some(cf_ws);
                        break;
                    }
                }
            }
        }
    }

    let ws = match ws {
        Some(w) => w,
        None => {
            return Err(format!("Could not connect to any upstream for DC{}", hs.dc_id).into());
        }
    };

    let splitter = Some(MsgSplitter::new(&relay_init, hs.proto_int));
    bridge_ws_reencrypt(stream, ws, ctx, splitter, stats, label).await?;

    Ok(())
}
