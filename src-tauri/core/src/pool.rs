use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::raw_websocket::RawWebSocket;
use tracing::{debug, info};

/// One pooled domain queue: connections plus their creation timestamps.
type DomainQueue = Arc<Mutex<VecDeque<(RawWebSocket, Instant)>>>;

/// A keep-alive pong must arrive within this window; otherwise the socket is
/// considered dead and dropped instead of being handed to a client.
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(3);

/// Hot-connection pool with per-domain locks.
///
/// The outer map is guarded by an `RwLock` and each domain queue has its own
/// `Mutex`, so connections to different Telegram DCs never block each other
/// (previously a single global async `Mutex` serialized all pool access).
pub struct ConnectionPool {
    domains: Arc<RwLock<HashMap<String, DomainQueue>>>,
    max_per_key: usize,
    io_buf: usize,
    max_age: Duration,
    keepalive: Duration,
}

impl ConnectionPool {
    pub fn new(max_per_key: usize, io_buf: usize, max_age_secs: u64, keepalive_secs: u64) -> Self {
        Self {
            domains: Arc::new(RwLock::new(HashMap::new())),
            max_per_key,
            io_buf: crate::net::clamp_io_buf(io_buf),
            max_age: Duration::from_secs(max_age_secs.max(10)),
            keepalive: Duration::from_secs(keepalive_secs),
        }
    }

    fn queue_for(&self, domain: &str) -> DomainQueue {
        // Fast path: read lock.
        {
            // `into_inner` on poison: a panicked task holding the map must not
            // turn every later connection into a panic too.
            let map = match self.domains.read() {
                Ok(m) => m,
                Err(p) => p.into_inner(),
            };
            if let Some(q) = map.get(domain) {
                return q.clone();
            }
        }
        // Slow path: create the queue once.
        let mut map = match self.domains.write() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        map.entry(domain.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(VecDeque::new())))
            .clone()
    }

    pub fn start_cleanup_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let max_age = self.max_age;
                // Snapshot queue handles under a short read lock, then prune
                // each queue under its own lock without holding the map.
                let queues: Vec<(String, DomainQueue)> = {
                    let map = match self.domains.read() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                };
                for (domain, q) in queues {
                    let mut list = match q.lock() {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    let before = list.len();
                    list.retain(|(_, created)| now.duration_since(*created) < max_age);
                    let expired = before - list.len();
                    if expired > 0 {
                        debug!("Pool cleanup: removed {} expired sockets for {}", expired, domain);
                    }
                }
            }
        });
    }

    /// Background keep-alive: every 15s send a real WS ping through each
    /// pooled connection and wait for the matching pong. Prevents
    /// Cloudflare/ISP idle timeouts from silently killing hot sockets
    /// (~100s idle window on some networks) and drops sockets that are
    /// already dead but still accept writes into the kernel buffer.
    ///
    /// Sockets are rotated one at a time (`pop_front` → ping → `push_back`)
    /// instead of moving the whole queue out: a client asking for a pooled
    /// socket during this window misses at most the one being pinged.
    pub fn start_keepalive_task(self: Arc<Self>) {
        if self.max_per_key == 0 || self.keepalive.is_zero() {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.keepalive);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let queues: Vec<(String, DomainQueue)> = {
                    let map = match self.domains.read() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                };
                for (domain, q) in queues {
                    let queued = match q.lock() {
                        Ok(l) => l.len(),
                        Err(_) => continue,
                    };
                    let mut dropped = 0usize;
                    for _ in 0..queued {
                        let item = match q.lock() {
                            Ok(mut l) => l.pop_front(),
                            Err(_) => break,
                        };
                        let Some((mut ws, created)) = item else {
                            break;
                        };
                        let alive = matches!(
                            tokio::time::timeout(KEEPALIVE_TIMEOUT, ws.ping_pong()).await,
                            Ok(Ok(()))
                        );
                        if alive {
                            if let Ok(mut l) = q.lock() {
                                l.push_back((ws, created));
                            }
                        } else {
                            dropped += 1;
                        }
                    }
                    if dropped > 0 {
                        debug!("Pool keep-alive: dropped {} dead sockets for {}", dropped, domain);
                    }
                }
            }
        });
    }

    pub async fn get(&self, domain: &str) -> Option<RawWebSocket> {
        if self.max_per_key == 0 {
            return None;
        }
        let q = self.queue_for(domain);
        let mut list = q.lock().ok()?;
        let now = Instant::now();
        while let Some((ws, created)) = list.pop_back() {
            if now.duration_since(created) < self.max_age {
                return Some(ws);
            }
            debug!("Pool hit expired socket for {} (>max_age), closing", domain);
        }
        None
    }

    pub async fn put(&self, domain: String, ws: RawWebSocket) {
        if self.max_per_key == 0 {
            return;
        }
        let mut overflow: Option<RawWebSocket> = None;
        {
            let q = self.queue_for(&domain);
            let mut list = match q.lock() {
                Ok(l) => l,
                Err(_) => return,
            };
            if list.len() < self.max_per_key {
                list.push_back((ws, Instant::now()));
            } else {
                overflow = Some(ws);
            }
        }
        if let Some(mut ws) = overflow {
            // Queue overflowed (e.g. several replenish tasks raced): the
            // socket did a full TCP+TLS+WS handshake that no client will
            // ever take — close it instead of silently leaking it to the
            // idle timeout. Closed outside the queue lock so awaiting the
            // close frame never blocks other pool users.
            let _ = ws.close().await;
        }
    }

    /// Queue length without blocking writers (best effort).
    fn len(&self, domain: &str) -> usize {
        let map = match self.domains.read() {
            Ok(m) => m,
            Err(_) => return 0,
        };
        map.get(domain)
            .and_then(|q| q.lock().ok().map(|l| l.len()))
            .unwrap_or(0)
    }

    /// Replenish one pooled connection in the background, but only when the
    /// queue has free capacity — this bounds the number of concurrent
    /// background connects (previously every pool hit spawned a task).
    pub async fn replenish(self: &Arc<Self>, domain: String, secure: bool) {
        if self.max_per_key == 0 || self.len(&domain) >= self.max_per_key {
            return;
        }
        let this = self.clone();
        let io_buf = self.io_buf;
        tokio::spawn(async move {
            match RawWebSocket::connect_with_buf(&domain, &domain, "/apiws", secure, io_buf).await {
                Ok(ws) => this.put(domain, ws).await,
                Err(e) => debug!("Pool replenish error for {}: {:?}", domain, e),
            }
        });
    }

    /// Pre-warm up to `min(pool_size, 4)` sockets per DC that actually has a
    /// WS front (DC2/DC4; DC1/3/5 answer 302 on `/apiws`, so warming them only
    /// burned timeouts). Media subdomains fall back to on-demand connect.
    pub async fn warm_up_defaults(&self, secure: bool) {
        if self.max_per_key == 0 {
            return;
        }
        let queues: Arc<RwLock<HashMap<String, DomainQueue>>> = self.domains.clone();
        let max = self.max_per_key;
        let io_buf = self.io_buf;
        tokio::spawn(async move {
            let mut targets = Vec::new();
            for dc in [2, 4] {
                targets.extend(crate::config::ws_domains(dc, false));
            }
            // Cap the eager warm-up: opening dozens of TLS+WS sessions at
            // startup is slow and looks like a traffic burst. The rest of the
            // pool fills on demand via `replenish`.
            let per_domain = max.min(4);
            for domain_str in targets {
                for _ in 0..per_domain {
                    match RawWebSocket::connect_with_buf(&domain_str, &domain_str, "/apiws", secure, io_buf).await {
                        Ok(ws) => {
                            let q = {
                                let mut map = match queues.write() {
                                    Ok(m) => m,
                                    Err(_) => break,
                                };
                                map.entry(domain_str.clone())
                                    .or_insert_with(|| Arc::new(Mutex::new(VecDeque::new())))
                                    .clone()
                            };
                            let mut list = match q.lock() {
                                Ok(l) => l,
                                Err(_) => break,
                            };
                            if list.len() < max {
                                list.push_back((ws, Instant::now()));
                                info!("Pre-warmed connection in pool for {}", domain_str);
                            }
                        }
                        Err(e) => {
                            // A failed warm-up usually means the network/DPI is
                            // blocking this DC; don't stall startup with more
                            // timeouts for the same domain.
                            tracing::debug!("Warm-up error for {}: {:?}", domain_str, e);
                            break;
                        }
                    }
                }
            }
        });
    }
}
