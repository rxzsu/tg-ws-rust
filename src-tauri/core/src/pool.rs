use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::raw_websocket::RawWebSocket;
use tracing::debug;
use tokio::{sync::Semaphore, task::JoinHandle};

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
    /// Bound on one background connect (replenish / warm-up). Without it a
    /// blackholed IP left the task hanging on the OS SYN timeout, and a
    /// burst of pool hits spawned several unbounded connects at once.
    connect_timeout: Duration,
    refills: Mutex<HashMap<String, Arc<Semaphore>>>,
    refill_tasks: Mutex<tokio::task::JoinSet<()>>,
}


impl ConnectionPool {
    pub fn new(
        max_per_key: usize,
        io_buf: usize,
        max_age_secs: u64,
        keepalive_secs: u64,
        connect_timeout_secs: u64,
    ) -> Self {
        Self {
            refills: Mutex::new(HashMap::new()),
            refill_tasks: Mutex::new(tokio::task::JoinSet::new()),
            domains: Arc::new(RwLock::new(HashMap::new())),
            max_per_key,
            io_buf: crate::net::clamp_io_buf(io_buf),
            max_age: Duration::from_secs(max_age_secs.max(10)),
            keepalive: Duration::from_secs(keepalive_secs),
            connect_timeout: Duration::from_secs(connect_timeout_secs.clamp(2, 120)),
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

    pub fn start_cleanup_task(self: Arc<Self>) -> JoinHandle<()> {
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
        })
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
    pub fn start_keepalive_task(self: Arc<Self>) -> Option<JoinHandle<()>> {
        if self.max_per_key == 0 || self.keepalive.is_zero() {
            return None;
        }
        Some(tokio::spawn(async move {
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
        }))
    }

    pub async fn get(&self, domain: &str) -> Option<RawWebSocket> {
        if self.max_per_key == 0 {
            return None;
        }
        let q = self.queue_for(domain);
        // Probe outside the queue lock: a write alone cannot detect an idle
        // connection blackholed by NAT/DPI. Bound the entire checkout, even
        // when several stale sockets are queued.
        tokio::time::timeout(KEEPALIVE_TIMEOUT, async {
            loop {
                let item = q.lock().ok()?.pop_back();
                let (mut ws, created) = item?;
                if created.elapsed() < self.max_age && ws.ping_pong().await.is_ok() {
                    return Some(ws);
                }
            }
        }).await.ok().flatten()
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
            let _ = tokio::time::timeout(Duration::from_secs(1), ws.close()).await;
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
        // One refill per domain in flight; queue length alone does not count
        // connections still handshaking, so a burst used to spawn unbounded work.
        let gate = {
            let mut refills = self.refills.lock().unwrap_or_else(|p| p.into_inner());
            refills.entry(domain.clone()).or_insert_with(|| Arc::new(Semaphore::new(1))).clone()
        };
        let Ok(permit) = gate.try_acquire_owned() else { return; };
        let this = Arc::downgrade(self);
        let io_buf = self.io_buf;
        let connect_timeout = self.connect_timeout;
        let mut tasks = self.refill_tasks.lock().unwrap_or_else(|p| p.into_inner());
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let _permit = permit;
            // Bounded like every client-facing connect: a blackholed domain
            // must not hold a background task for the OS SYN timeout.
            match tokio::time::timeout(
                connect_timeout,
                RawWebSocket::connect_with_buf(&domain, &domain, "/apiws", secure, io_buf),
            )
            .await
            {
                Ok(Ok(ws)) => {
                    if let Some(pool) = this.upgrade() { pool.put(domain, ws).await; }
                }
                Ok(Err(e)) => debug!("Pool replenish error for {}: {:?}", domain, e),
                Err(_) => debug!("Pool replenish timeout for {}", domain),
            }
        });
    }

    /// Pre-warm up to `min(pool_size, 4)` sockets per DC that actually has a
    /// WS front (DC2/DC4; DC1/3/5 answer 302 on `/apiws`, so warming them only
    /// burned timeouts). Media subdomains fall back to on-demand connect.
    pub fn warm_up_defaults(self: Arc<Self>, secure: bool) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            for dc in [2, 4] {
                for domain in crate::config::ws_domains(dc, false) {
                    let pool = self.clone();
                    tasks.spawn(async move {
                        // Parallel across domains/DCs, conservative within one
                        // domain so an unavailable DC cannot delay warming another.
                        for _ in 0..pool.max_per_key.min(4) {
                            match tokio::time::timeout(pool.connect_timeout,
                                RawWebSocket::connect_with_buf(&domain, &domain, "/apiws", secure, pool.io_buf)
                            ).await {
                                Ok(Ok(ws)) => pool.put(domain.clone(), ws).await,
                                _ => break,
                            }
                        }
                    });
                }
            }
            while tasks.join_next().await.is_some() {}
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn dead_pooled_socket_is_discarded_before_relay_init() {
        let pool = ConnectionPool::new(2, 4096, 60, 0, 2);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, server) = tokio::join!(tokio::net::TcpStream::connect(listener.local_addr().unwrap()), listener.accept());
        let mut server = server.unwrap().0;
        // A peer that closed its WS session can still accept a kernel write.
        server.write_all(&[0x88, 0]).await.unwrap();
        pool.put("test".into(), RawWebSocket::from_test_stream(client.unwrap())).await;
        assert!(pool.get("test").await.is_none());
        assert_eq!(pool.len("test"), 0);
    }

    #[tokio::test]
    async fn stopped_pool_releases_maintenance_references() {
        let pool = Arc::new(ConnectionPool::new(2, 4096, 60, 15, 2));
        let weak = Arc::downgrade(&pool);
        let mut tasks = crate::upstream::BackgroundTasks(vec![pool.clone().start_cleanup_task()]);
        tasks.0.extend(pool.clone().start_keepalive_task());
        tokio::task::yield_now().await;
        drop(pool);
        drop(tasks);
        tokio::task::yield_now().await;
        assert!(weak.upgrade().is_none(), "old bridge must not retain a pool after stop");
    }
}
