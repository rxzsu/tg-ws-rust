pub mod balancer;
pub mod bridge;
pub mod bufpool;
pub mod config;
pub mod crypto;
pub mod ech;
pub mod fake_tls;
pub mod h3;
pub mod handshake;
pub mod logging;
pub mod net;
pub mod pool;
pub mod quic;
pub mod raw_websocket;
pub mod splitter;
pub mod stats;
pub mod version;

use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tracing::{error, info, warn};

use crate::balancer::Balancer;
use crate::bridge::{bridge_tcp_fallback, bridge_ws_reencrypt, BridgeOptions, RemoteIo};
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

/// A client that connects but never completes the MTProto handshake keeps a
/// task and a socket busy forever; bound the whole pre-bridge exchange.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A bridge session with no traffic in either direction for this long is a
/// half-dead socket (NAT rebinding, silent drop). Telegram clients exchange
/// pings well within this window, so live sessions are never cut.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

/// Millis of the last observed session activity (epoch), updated on every
/// read in both bridge directions. Drives the idle watchdog cheaply.
use crate::bridge::ActivityStamp;

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve into `true` only on a real shutdown signal. `Lagged` means this
/// receiver missed messages because of its own slow polling — it is not a
/// stop request and previously killed healthy sessions.
async fn shutdown_signal(rx: &mut tokio::sync::broadcast::Receiver<()>) -> bool {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match rx.recv().await {
            Ok(()) => return true,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return true,
        }
    }
}

/// Any pre-bridge read bounded by [`HANDSHAKE_TIMEOUT`] so a silent client
/// cannot pin the task forever.
async fn bounded_io<F, T>(fut: F) -> std::io::Result<T>
where
    F: std::future::Future<Output = std::io::Result<T>>,
{
    tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake read timeout"))?
}

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
    if let Err(msg) = crate::config::validate(&config) {
        error!("{}", msg);
        return Err(msg.into());
    }

    // The dependency tree contains both `ring` (quinn) and `aws-lc-rs`, so
    // rustls cannot pick a provider on its own and panics on first use. The
    // GUI binary installs one in `main`, but the core must be safe to embed
    // anywhere — the balancer's background TLS probes hit this immediately.
    crate::quic::ensure_crypto_provider();
    crate::ech::set_enabled(config.ech_enabled);

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

    // `validate` already proved this decodes to 16 bytes; a silent fallback to
    // an all-zero key would look like a working proxy that never connects.
    let secret_bytes = hex::decode(config.secret.trim())
        .map_err(|_| "Секрет должен быть 32 hex-символа (16 байт)")?;
    stats.mark_started();
    let secret = Arc::new(secret_bytes);
    let cfg = Arc::new(config);
    let pool = Arc::new(ConnectionPool::new(
        cfg.pool_size,
        crate::net::clamp_io_buf(cfg.buffer_size),
        cfg.pool_max_age_secs,
        cfg.pool_keepalive_secs,
    ));
    pool.clone().start_cleanup_task();
    pool.clone().start_keepalive_task();
    if cfg.pool_size > 0 {
        pool.warm_up_defaults(!cfg.disable_secure).await;
    }

    let balancer = Arc::new(Balancer::with_builtins(cfg.use_builtin_cf_domains));
    balancer.clone().start_background_refresh();

    // Active sessions are tracked so a stop can drain them instead of
    // dropping the handler futures mid-write (which used to cut the last
    // packet and skip the WebSocket close frame).
    let mut sessions: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

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
            _ = shutdown_signal(&mut shutdown_rx) => {
                info!("Server shutting down gracefully");
                break;
            }
            // Reap finished sessions so the set does not grow unbounded.
            Some(_) = sessions.join_next() => {
                continue;
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

        sessions.spawn(async move {
            stats.inc_total();
            stats.inc_active();
            let label = peer_addr.to_string();

            let activity: ActivityStamp = Arc::new(std::sync::atomic::AtomicU64::new(now_millis()));
            let mut handler = Box::pin(handle_connection(
                stream,
                secret,
                stats.clone(),
                cfg,
                pool,
                balancer,
                label.clone(),
                activity.clone(),
            ));

            let idle = tokio::time::sleep(IDLE_TIMEOUT);
            tokio::pin!(idle);

            loop {
                tokio::select! {
                    res = &mut handler => {
                        if let Err(e) = res {
                            warn!("[{}] connection error: {:?}", label, e);
                            // Surfaced on the dashboard: without this the UI just
                            // shows "running" while Telegram silently fails.
                            stats.set_last_error(format!("{}: {}", label, e));
                        }
                        break;
                    }
                    _ = shutdown_signal(&mut conn_shutdown) => {
                        // Let in-flight writes flush and the close frame go out
                        // instead of dropping the future on the spot.
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_millis(150),
                            &mut handler,
                        )
                        .await;
                        info!("[{}] connection closed due to proxy stop", label);
                        break;
                    }
                    _ = &mut idle => {
                        // Watchdog: half-dead sessions with no traffic in either
                        // direction are reaped; live ones reset the timer.
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        let last = activity.load(std::sync::atomic::Ordering::Relaxed);
                        if now_millis().saturating_sub(last) >= IDLE_TIMEOUT.as_millis() as u64 {
                            warn!("[{}] connection idle for {}s, closing", label, IDLE_TIMEOUT.as_secs());
                            break;
                        }
                    }
                }
            }
            stats.dec_active();
        });
    }

    // Stop accepting, then give live sessions a short window to finish.
    drop(listener);
    let aborted = drain_sessions(&mut sessions, std::time::Duration::from_millis(1000)).await;
    if aborted > 0 {
        info!("Aborted {} session(s) that were still active after the grace period", aborted);
    }
    Ok(())
}

/// Wait for tracked sessions to finish, aborting whatever is still running
/// after `grace`. Returns how many sessions had to be aborted.
async fn drain_sessions(
    sessions: &mut tokio::task::JoinSet<()>,
    grace: std::time::Duration,
) -> usize {
    let mut alive = sessions.len();
    let deadline = tokio::time::sleep(grace);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                sessions.shutdown().await;
                break;
            }
            joined = sessions.join_next() => {
                if joined.is_none() {
                    break;
                }
                alive = alive.saturating_sub(1);
                if alive == 0 {
                    break;
                }
            }
        }
    }
    alive
}

pub async fn run_server(config: ProxyConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stats = Arc::new(Stats::new());
    let (_tx, rx) = tokio::sync::broadcast::channel(1);
    run_server_with_stats(config, stats, rx).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    mut stream: TcpStream,
    secret: Arc<Vec<u8>>,
    stats: Arc<Stats>,
    cfg: Arc<ProxyConfig>,
    pool: Arc<ConnectionPool>,
    balancer: Arc<Balancer>,
    label: String,
    activity: ActivityStamp,
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

        if let Ok(true) = read_res
            && let Ok(pp_str) = std::str::from_utf8(&pp_buf)
        {
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

    let mut first_byte = [0u8; 1];
    bounded_io(stream.read_exact(&mut first_byte)).await?;

    let (handshake_buf, client_reader, client_writer) = if !cfg.fake_tls_domain.is_empty() {
        if first_byte[0] == TLS_RECORD_HANDSHAKE {
            // Fake TLS client hello
            let mut rest_hdr = [0u8; 4];
            bounded_io(stream.read_exact(&mut rest_hdr)).await?;
            let record_len = u16::from_be_bytes([rest_hdr[2], rest_hdr[3]]) as usize;
            let mut record_body = vec![0u8; record_len];
            bounded_io(stream.read_exact(&mut record_body)).await?;

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
            bounded_io(clt_r.read_exact(&mut inner_buf)).await?;
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
        bounded_io(stream.read_exact(&mut rest)).await?;
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

    // Telegram only fronts `/apiws` on DC2/DC4. Attempting DC1/3/5 anyway
    // burns a full `connect_timeout` of dead air per retry (their IPs are
    // DPI-blackholed) and leaves the client in a reconnect loop, so skip
    // straight to the fallbacks and say so.
    if domains.is_empty() {
        const NO_FRONT: &str = "DC1/3/5 не имеют WS-фронта: /apiws отвечает 302, \
             MTProto-TCP к их IP заблокирован провайдером";
        stats.record_dc_route(dc_id, "no_front", NO_FRONT);
        warn!(
            "[{}] DC{} has no WebSocket front (telegram.org/apiws answers 302 there); \
             going straight to fallbacks",
            label, dc_id
        );
    }

    /// How many health-ordered domains are connected to in parallel; the
    /// first successful handshake wins. Happy-Eyeballs for the case where
    /// one CDN front is blocked or degraded.
    const RACE_WIDTH: usize = 3;

    // Pooled sockets first: zero network cost, try every domain's queue.
    for domain in &domains {
        let attempt_start = std::time::Instant::now();
        if let Some(mut pooled) = pool.get(domain).await {
            // Auto-replenish pool in background so next client connection also gets 0ms latency
            // (bounded: replenish() skips when the queue is already full).
            pool.replenish(domain.clone(), !cfg.disable_secure).await;

            if pooled.send(&relay_init).await.is_ok() {
                stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                balancer.record_success(domain, attempt_start.elapsed().as_micros()).await;
                ws = Some(pooled);
                break;
            }
            // Pooled socket was dead on arrival: drop it and deprioritize the
            // domain before the fresh-connect attempt below.
            balancer.record_failure(domain).await;
        }
    }

    // Fresh connects: race the top candidates in parallel instead of paying
    // a full serial timeout per dead front.
    if ws.is_none() && !domains.is_empty() {
        let ct = std::time::Duration::from_secs(cfg.connect_timeout_secs.clamp(2, 120));
        let candidates = &domains[..domains.len().min(RACE_WIDTH)];
        type ConnectResult = Result<RawWebSocket, Box<dyn std::error::Error + Send + Sync>>;
        let mut set: tokio::task::JoinSet<(String, Option<ConnectResult>)> =
            tokio::task::JoinSet::new();
        for domain in candidates {
            let domain = domain.clone();
            let secure = !cfg.disable_secure;
            set.spawn(async move {
                match tokio::time::timeout(
                    ct,
                    RawWebSocket::connect_with_buf(&domain, &domain, "/apiws", secure, io_buf),
                )
                .await
                {
                    Ok(res) => (domain, Some(res)),
                    Err(_) => (domain, None),
                }
            });
        }

        let mut winner: Option<(String, RawWebSocket)> = None;
        while let Some(joined) = set.join_next().await {
            let Ok((domain, res)) = joined else { continue };
            match res {
                Some(Ok(socket)) => {
                    info!("[{}] Race winner: upstream WS {}", label, domain);
                    winner = Some((domain, socket));
                    break;
                }
                Some(Err(e)) => {
                    warn!("[{}] Race: connection to {} failed: {:?}", label, domain, e);
                    balancer.record_failure(&domain).await;
                    stats.record_dc_route(dc_id, "failed", format!("WS {}: {}", domain, e));
                }
                None => {
                    warn!("[{}] Race: connection to {} timed out", label, domain);
                    balancer.record_failure(&domain).await;
                    stats.record_dc_route(dc_id, "timeout", format!("WS {}: таймаут", domain));
                }
            }
        }
        // Losers still connecting are cancelled; sockets whose handshake had
        // already completed are simply dropped (TCP close), the same as a
        // failed attempt.
        set.abort_all();

        if let Some((domain, mut socket)) = winner {
            let attempt_start = std::time::Instant::now();
            if socket.send(&relay_init).await.is_ok() {
                stats.record_dc_route(dc_id, "ok", format!("WS {}", domain));
                stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                balancer.record_success(&domain, attempt_start.elapsed().as_micros()).await;
                ws = Some(socket);
            } else {
                warn!("[{}] Race winner {} refused relay init", label, domain);
                balancer.record_failure(&domain).await;
            }
        }

        // Racing covered only the first RACE_WIDTH domains; fall back to a
        // serial pass over the rest so a large domain list still gets tried.
        if ws.is_none() && domains.len() > RACE_WIDTH {
            for domain in &domains[RACE_WIDTH..] {
                let attempt_start = std::time::Instant::now();
                info!("[{}] Connecting to upstream WS {}", label, domain);
                let conn_res = tokio::time::timeout(
                    ct,
                    RawWebSocket::connect_with_buf(domain, domain, "/apiws", !cfg.disable_secure, io_buf),
                )
                .await;
                match conn_res {
                    Ok(Ok(mut socket)) => {
                        if let Err(e) = socket.send(&relay_init).await {
                            warn!("[{}] Failed to send relay init to {}: {:?}", label, domain, e);
                            balancer.record_failure(domain).await;
                            continue;
                        }
                        stats.record_dc_route(dc_id, "ok", format!("WS {}", domain));
                        stats.connections_ws.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        balancer.record_success(domain, attempt_start.elapsed().as_micros()).await;
                        ws = Some(socket);
                        break;
                    }
                    Ok(Err(e)) => {
                        warn!("[{}] Connection to {} failed: {:?}", label, domain, e);
                        balancer.record_failure(domain).await;
                        stats.record_dc_route(dc_id, "failed", format!("WS {}: {}", domain, e));
                    }
                    Err(_) => {
                        warn!("[{}] Connection to {} timed out", label, domain);
                        balancer.record_failure(domain).await;
                        stats.record_dc_route(dc_id, "timeout", format!("WS {}: таймаут", domain));
                    }
                }
            }
        }
    }

    if ws.is_none() {
        // Every direct domain failed: move this DC's sticky pick to the next
        // healthiest one so the next connection does not start on a dead node.
        balancer.rotate_domain_for_dc(dc_id).await;
    }

    // 1b. Relay transports (experimental): only when relays are configured.
    // Telegram DCs don't speak QUIC, so without user relays this step is
    // skipped entirely (zero behavior change by default).
    //   `quic` → custom ALPN, raw bidirectional stream
    //   `h3`   → real HTTP/3, MTProto is the body of POST /mtproto
    //   `auto` → try both, QUIC first
    if ws.is_none() && !cfg.quic_relays.is_empty() {
        use tokio::io::AsyncWriteExt;
        let ct = std::time::Duration::from_secs(cfg.connect_timeout_secs.clamp(2, 120));
        let want_quic = matches!(cfg.transport.as_str(), "auto" | "quic");
        let want_h3 = matches!(cfg.transport.as_str(), "auto" | "h3");

        if want_quic {
            for relay in &cfg.quic_relays {
                info!("[{}] QUIC relay: {}", label, relay);
                match crate::quic::connect_quic(relay, ct).await {
                    Ok(mut up) => {
                        if up.send.write_all(&relay_init).await.is_ok() {
                            stats
                                .connections_quic
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            info!("[{}] QUIC relay {} connected", label, relay);
                            return bridge_tcp_fallback(
                                client_reader,
                                client_writer,
                                RemoteIo {
                                    reader: up.recv,
                                    writer: up.send,
                                },
                                ctx,
                                stats,
                                label,
                                io_buf,
                                activity,
                            )
                            .await;
                        }
                        warn!("[{}] QUIC relay {} init failed", label, relay);
                    }
                    Err(e) => {
                        warn!("[{}] QUIC relay {} failed: {:?}", label, relay, e);
                    }
                }
            }
        }

        if want_h3 {
            for relay in &cfg.quic_relays {
                info!("[{}] HTTP/3 relay: {}", label, relay);
                match crate::h3::connect_h3(relay, ct).await {
                    Ok(mut up) => {
                        if up.writer.write_all(&relay_init).await.is_ok() {
                            stats
                                .connections_h3
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            info!("[{}] HTTP/3 relay {} connected", label, relay);
                            return bridge_tcp_fallback(
                                client_reader,
                                client_writer,
                                RemoteIo {
                                    reader: up.reader,
                                    writer: up.writer,
                                },
                                ctx,
                                stats,
                                label,
                                io_buf,
                                activity,
                            )
                            .await;
                        }
                        warn!("[{}] HTTP/3 relay {} init failed", label, relay);
                    }
                    Err(e) => {
                        warn!("[{}] HTTP/3 relay {} failed: {:?}", label, relay, e);
                    }
                }
            }
        }
    }

    // 2. Fallback: CF Worker / CF Domains / Dynamic Balancer if direct WS failed
    if ws.is_none() && cfg.fallback_cfproxy {
        // Same bounded-attempt window the direct path above uses.
        let ct = std::time::Duration::from_secs(cfg.connect_timeout_secs.clamp(2, 120));
        let fallback_ip = get_default_dc_ip(dc_id, is_test);

        // Try CF Worker domains
        if let Some(ip) = fallback_ip {
            for worker_domain in &cfg.cfproxy_worker_domains {
                let media_flag = if hs.is_media { 1 } else { 0 };
                let path = format!("/apiws?dst={}&dc={}&media={}", ip, hs.dc_id, media_flag);
                info!("[{}] Fallback CF Worker: {} -> {}", label, worker_domain, path);
                // Bounded like the direct path above: a hung handshake to one
                // fallback domain must not stall the whole fallback chain.
                let conn = tokio::time::timeout(ct, RawWebSocket::connect_with_buf(worker_domain, worker_domain, &path, !cfg.disable_secure, io_buf)).await;
                if let Ok(Ok(mut worker_ws)) = conn
                    && worker_ws.send(&relay_init).await.is_ok()
                {
                    stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ws = Some(worker_ws);
                    break;
                }
            }
        }

        // Try User CF domains
        if ws.is_none() {
            for cf_domain in &cfg.cfproxy_user_domains {
                info!("[{}] Fallback User CF Domain: {}", label, cf_domain);
                let conn = match tokio::time::timeout(ct, RawWebSocket::connect_with_buf(cf_domain, cf_domain, "/apiws", !cfg.disable_secure, io_buf)).await {
                    Ok(r) => r,
                    Err(_) => {
                        warn!("[{}] Fallback User CF Domain {} timed out", label, cf_domain);
                        continue;
                    }
                };
                if let Ok(mut cf_ws) = conn
                    && cf_ws.send(&relay_init).await.is_ok()
                {
                    stats.connections_cfproxy.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ws = Some(cf_ws);
                    break;
                }
            }
        }

        // Try Dynamic Balancer Domains
        if ws.is_none() {
            let balancer_domains = balancer.get_domains_for_dc(dc_id).await;
            for b_domain in balancer_domains {
                let host = format!("kws{}.{}", dc_id, b_domain);
                info!("[{}] Fallback Balancer Domain: {}", label, host);
                let conn = match tokio::time::timeout(ct, RawWebSocket::connect_with_buf(&host, &host, "/apiws", !cfg.disable_secure, io_buf)).await {
                    Ok(r) => r,
                    Err(_) => {
                        warn!("[{}] Fallback Balancer Domain {} timed out", label, host);
                        balancer.record_failure(&b_domain).await;
                        continue;
                    }
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
    if ws.is_none()
        && let Some(target_ip) = get_default_dc_ip(dc_id, is_test)
    {
        info!("[{}] All WebSockets failed, engaging Direct TCP Fallback to {}:443", label, target_ip);
        // A blackholed IP never answers, so waiting the full connect timeout
        // here only adds dead air to a route that is already doomed. Probe
        // briefly instead; the fallback chain has better options behind it.
        let probe_timeout = std::time::Duration::from_secs(
            cfg.connect_timeout_secs.clamp(1, 3),
        );
        let tcp_conn = tokio::time::timeout(
            probe_timeout,
            crate::net::tcp_connect(target_ip, 443, io_buf)
        ).await;

        if let Ok(Ok(mut remote_stream)) = tcp_conn {
            use tokio::io::AsyncWriteExt;
            if remote_stream.write_all(&relay_init).await.is_ok() && remote_stream.flush().await.is_ok() {
                stats.record_dc_route(dc_id, "ok", format!("TCP {}:443", target_ip));
                stats.connections_tcp.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                info!("[{}] Direct TCP Fallback connected to {}:443", label, target_ip);
                let (remote_reader, remote_writer) = remote_stream.into_split();
                return bridge_tcp_fallback(client_reader, client_writer, RemoteIo { reader: remote_reader, writer: remote_writer }, ctx, stats, label, io_buf, activity).await;
            }
        } else {
            warn!("[{}] Direct TCP Fallback connection to {}:443 failed", label, target_ip);
            stats.record_dc_route(
                dc_id,
                "failed",
                format!("MTProto-TCP {}:443 заблокирован (нет ответа)", target_ip),
            );
        }
    }

    let ws = match ws {
        Some(w) => w,
        None => {
            let detail = if !crate::config::has_ws_front(dc_id) {
                format!(
                    "DC{} недоступен: WS-фронта /apiws у него нет, а MTProto-TCP к {} \
                     блокируется провайдером. Файлы и медиа с этой DC не загрузятся — \
                     добавьте релей (домен на Cloudflare, Worker или QUIC/H3) или другой канал связи",
                    dc_id,
                    get_default_dc_ip(dc_id, is_test).unwrap_or("DC-IP")
                )
            } else {
                format!("все апстримы для DC{} (WS, CF, TCP) недоступны", dc_id)
            };
            stats.record_dc_route(dc_id, "failed", &detail);
            stats.set_last_error(format!("DC{}: {}", dc_id, detail));
            return Err(format!("Не удалось подключиться к DC{}: {}", hs.dc_id, detail).into());
        }
    };

    let splitter = if cfg.splitter_enabled {
        Some(MsgSplitter::new(&relay_init, hs.proto_int))
    } else {
        None
    };
    bridge_ws_reencrypt(
        client_reader,
        client_writer,
        ws,
        ctx,
        BridgeOptions { splitter, io_buf },
        stats,
        label,
        activity,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod drain_tests {
    use super::drain_sessions;
    use std::time::Duration;

    #[tokio::test]
    async fn drains_finished_sessions_within_grace() {
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..2 {
            set.spawn(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
            });
        }
        let aborted = drain_sessions(&mut set, Duration::from_millis(500)).await;
        assert_eq!(aborted, 0, "both sessions finished within the grace window");
        assert!(set.is_empty());
    }

    #[tokio::test]
    async fn aborts_stuck_sessions_after_grace() {
        let mut set = tokio::task::JoinSet::new();
        set.spawn(async {
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let aborted = drain_sessions(&mut set, Duration::from_millis(50)).await;
        assert_eq!(aborted, 1, "the stuck session must be aborted");
        assert!(started.elapsed() < Duration::from_secs(2), "drain must not hang");
        assert!(set.is_empty(), "stuck sessions must be aborted");
    }
}
