pub mod balancer;
pub mod bridge;
pub mod cf_h2;
pub mod config;
pub mod crypto;
pub mod fake_tls;
pub mod handshake;
pub mod logging;
pub mod net;
pub mod pool;
pub mod raw_websocket;
pub mod splitter;
pub mod stats;

use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tracing::{error, info, warn};

use crate::balancer::Balancer;
use crate::bridge::{bridge_tcp_fallback, bridge_ws_reencrypt};
use crate::config::{get_default_dc_ip, ProxyConfig};
use crate::crypto::CryptoCtx;
use crate::fake_tls::{
    build_server_hello, proxy_to_masking_domain, verify_client_hello, ClientReader, ClientWriter,
    FakeTlsReader, FakeTlsWriter, TLS_RECORD_HANDSHAKE,
};
use crate::handshake::{generate_relay_init, try_handshake};
use crate::pool::ConnectionPool;
use crate::raw_websocket::RawWebSocket;
use crate::splitter::MsgSplitter;
pub use crate::stats::{Stats, TelemetrySnapshot};

pub fn diagnose_bind_error(err: &std::io::Error, host: &str, port: u16) -> String {
    let err_kind = err.kind();
    let raw_os = err.raw_os_error().unwrap_or(0);

    if err_kind == std::io::ErrorKind::AddrInUse || raw_os == 10048 || raw_os == 98 {
        format!(
            "Порт {} уже занят другим приложением. Выберите другой порт в настройках.",
            port
        )
    } else if err_kind == std::io::ErrorKind::PermissionDenied || raw_os == 10013 || raw_os == 13 {
        format!(
            "Доступ к порту {} заблокирован системой или брандмауэром. Запустите программу от имени администратора или выберите порт выше 1024.",
            port
        )
    } else if err_kind == std::io::ErrorKind::AddrNotAvailable || raw_os == 10049 || raw_os == 99 {
        format!(
            "IP-адрес {} недоступен на данном сетевом интерфейсе. Используйте 127.0.0.1 или 0.0.0.0.",
            host
        )
    } else {
        format!("Не удалось запустить прокси на {}:{}: {}", host, port, err)
    }
}

pub async fn run_server_with_stats(
    config: ProxyConfig,
    stats: Arc<Stats>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("{}:{}", config.host, config.port);
    let listener = match crate::net::bind_listener(&config.host, config.port).await {
        Ok(l) => l,
        Err(e) => {
            let msg = diagnose_bind_error(&e, &config.host, config.port);
            error!("{}", msg);
            return Err(msg.into());
        }
    };
    info!("tg-ws-proxy listening on {}", addr);

    let secret_bytes = hex::decode(&config.secret).unwrap_or_else(|_| vec![0u8; 16]);
    let secret = Arc::new(secret_bytes);
    let cfg = Arc::new(config);
    let pool = Arc::new(ConnectionPool::new(
        cfg.pool_size.max(1),
        crate::net::clamp_io_buf(cfg.buffer_size),
        cfg.pool_max_age_secs,
    ));
    pool.clone().start_cleanup_task();
    if cfg.pool_size > 0 {
        pool.warm_up_defaults(!cfg.disable_secure).await;
    }

    let balancer = Arc::new(Balancer::new());
    balancer.clone().start_background_refresh();

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
        let balancer = balancer.clone();
        let mut conn_shutdown = shutdown_rx.resubscribe();

        // Apply kernel socket buffers from config (best effort).
        let io_buf = crate::net::clamp_io_buf(cfg.buffer_size);
        let stream = match crate::net::tune_accepted(stream, io_buf) {
            Ok(s) => s,
            Err(e) => {
                warn!("[{}] socket tuning failed, dropping connection: {:?}", peer_addr, e);
                continue;
            }
        };

        tokio::spawn(async move {
            stats.inc_total();
            stats.inc_active();
            let label = peer_addr.to_string();

            tokio::select! {
                res = handle_connection(stream, secret, stats.clone(), cfg, pool, balancer, label.clone()) => {
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
    balancer: Arc<Balancer>,
    label: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = stream.set_nodelay(true);

    let mut label = label;

    // 1. PROXY Protocol v1 support
    if cfg.proxy_protocol {
        let mut pp_buf = Vec::with_capacity(108);
        let mut byte = [0u8; 1];
        let read_res = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while pp_buf.len() < 108 {
                if stream.read_exact(&mut byte).await.is_err() {
                    return false;
                }
                pp_buf.push(byte[0]);
                if pp_buf.ends_with(b"\r\n") {
                    return true;
                }
            }
            false
        }).await;

        if let Ok(true) = read_res {
            if let Ok(pp_str) = std::str::from_utf8(&pp_buf) {
                let trimmed = pp_str.trim();
                if trimmed.starts_with("PROXY ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 6 {
                        label = format!("{}:{}", parts[2], parts[4]);
                        tracing::debug!("[{}] PROXY protocol v1 client: {}", label, trimmed);
                    }
                }
            }
        }
    }

    let mut first_byte = [0u8; 1];
    stream.read_exact(&mut first_byte).await?;

    let (handshake_buf, client_reader, client_writer) = if !cfg.fake_tls_domain.is_empty() {
        if first_byte[0] == TLS_RECORD_HANDSHAKE {
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
                None => {
                    info!(
                        "[{}] Fake TLS verification failed -> proxying to real site {}",
                        label, cfg.fake_tls_domain
                    );
                    stats.connections_bad.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let _ = proxy_to_masking_domain(stream, &client_hello, &cfg.fake_tls_domain, &label).await;
                    return Ok(());
                }
            };

            let sh = build_server_hello(&secret, &tls_res.client_random, &tls_res.session_id);
            tokio::io::AsyncWriteExt::write_all(&mut stream, &sh).await?;
            tokio::io::AsyncWriteExt::flush(&mut stream).await?;

            let (cr, cw) = stream.into_split();
            let mut clt_r = ClientReader::FakeTls(FakeTlsReader::new(cr));
            let clt_w = ClientWriter::FakeTls(FakeTlsWriter::new(cw));

            // Read inner obfs2 handshake from TLS stream
            let mut inner_buf = [0u8; 64];
            clt_r.read_exact(&mut inner_buf).await?;
            (inner_buf, clt_r, clt_w)
        } else {
            // Non-TLS byte on Fake TLS domain -> HTTP 301 Moved Permanently redirect
            tracing::debug!(
                "[{}] Non-TLS byte 0x{:02X} -> 301 redirect to {}",
                label, first_byte[0], cfg.fake_tls_domain
            );
            let redirect = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: https://{}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                cfg.fake_tls_domain
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, redirect.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::flush(&mut stream).await;
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
            return Ok(());
        }
    } else {
        let mut rest = [0u8; 63];
        stream.read_exact(&mut rest).await?;
        let mut full = [0u8; 64];
        full[0] = first_byte[0];
        full[1..].copy_from_slice(&rest);

        let (cr, cw) = stream.into_split();
        (full, ClientReader::Plain(cr), ClientWriter::Plain(cw))
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

    let io_buf = crate::net::clamp_io_buf(cfg.buffer_size);

    // 1. Try WebSocket Domains with connection pooling & background replenishment
    let domains = crate::config::ws_domains(dc_id, hs.is_media);
    let mut ws: Option<RawWebSocket> = None;

    for domain in &domains {
        // Pool check
        if let Some(mut pooled) = pool.get(domain).await {
            // Auto-replenish pool in background so next client connection also gets 0ms latency
            // (bounded: replenish() skips when the queue is already full).
            pool.replenish(domain.clone(), !cfg.disable_secure).await;

            if pooled.send(&relay_init).await.is_ok() {
                stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ws = Some(pooled);
                break;
            }
        }

        info!("[{}] Connecting to upstream WS {}", label, domain);
        let conn_res = match RawWebSocket::connect_with_buf(domain, domain, "/apiws", !cfg.disable_secure, io_buf).await {
            Ok(s) => Ok(s),
            Err(_) => RawWebSocket::connect_with_sni(domain, domain, "/apiws", !cfg.disable_secure, Some("sprinthost.ru"), io_buf).await,
        };

        match conn_res {
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

    // 2. Fallback: CF Worker / CF Domains / Dynamic Balancer if direct WS failed
    if ws.is_none() && cfg.fallback_cfproxy {
        let fallback_ip = get_default_dc_ip(dc_id, is_test);

        // Try CF Worker domains
        if let Some(ip) = fallback_ip {
            for worker_domain in &cfg.cfproxy_worker_domains {
                let media_flag = if hs.is_media { 1 } else { 0 };
                let path = format!("/apiws?dst={}&dc={}&media={}", ip, hs.dc_id, media_flag);
                info!("[{}] Fallback CF Worker: {} -> {}", label, worker_domain, path);
                if let Ok(mut worker_ws) = RawWebSocket::connect_with_buf(worker_domain, worker_domain, &path, !cfg.disable_secure, io_buf).await {
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
                let conn = match RawWebSocket::connect_with_buf(cf_domain, cf_domain, "/apiws", !cfg.disable_secure, io_buf).await {
                    Ok(s) => Ok(s),
                    Err(_) => RawWebSocket::connect_with_sni(cf_domain, cf_domain, "/apiws", !cfg.disable_secure, Some("sprinthost.ru"), io_buf).await,
                };
                if let Ok(mut cf_ws) = conn {
                    if cf_ws.send(&relay_init).await.is_ok() {
                        stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        ws = Some(cf_ws);
                        break;
                    }
                }
            }
        }

        // Try Dynamic Balancer Domains
        if ws.is_none() {
            let balancer_domains = balancer.get_domains_for_dc(dc_id).await;
            for b_domain in balancer_domains {
                let host = format!("kws{}.{}", dc_id, b_domain);
                info!("[{}] Fallback Balancer Domain: {}", label, host);
                let conn = match RawWebSocket::connect_with_buf(&host, &host, "/apiws", !cfg.disable_secure, io_buf).await {
                    Ok(s) => Ok(s),
                    Err(_) => RawWebSocket::connect_with_sni(&host, &host, "/apiws", !cfg.disable_secure, Some("sprinthost.ru"), io_buf).await,
                };

                if let Ok(mut b_ws) = conn {
                    if b_ws.send(&relay_init).await.is_ok() {
                        stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        ws = Some(b_ws);
                        break;
                    }
                } else {
                    balancer.rotate_domain_for_dc(dc_id).await;
                }
            }
        }
    }

    // 3. Direct TCP Fallback to Telegram DC IP if all WebSockets failed
    if ws.is_none() {
        if let Some(target_ip) = get_default_dc_ip(dc_id, is_test) {
            info!("[{}] All WebSockets failed, engaging Direct TCP Fallback to {}:443", label, target_ip);
            let tcp_conn = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::net::tcp_connect(target_ip, 443, io_buf)
            ).await;

            if let Ok(Ok(mut remote_stream)) = tcp_conn {
                use tokio::io::AsyncWriteExt;
                if remote_stream.write_all(&relay_init).await.is_ok() && remote_stream.flush().await.is_ok() {
                    stats.connections_tcp.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    info!("[{}] Direct TCP Fallback connected to {}:443", label, target_ip);
                    return bridge_tcp_fallback(client_reader, client_writer, remote_stream, ctx, stats, label, io_buf).await;
                }
            } else {
                warn!("[{}] Direct TCP Fallback connection to {}:443 failed", label, target_ip);
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
    bridge_ws_reencrypt(client_reader, client_writer, ws, ctx, splitter, stats, label, io_buf).await?;

    Ok(())
}
