use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::raw_websocket::RawWebSocket;
use tracing::{debug, info};

pub fn default_max_age() -> Duration {
    Duration::from_secs(120)
}

/// Hot-connection pool with per-domain locks.
///
/// The outer map is guarded by an `RwLock` and each domain queue has its own
/// `Mutex`, so connections to different Telegram DCs never block each other
/// (previously a single global async `Mutex` serialized all pool access).
pub struct ConnectionPool {
    domains: Arc<RwLock<HashMap<String, Arc<Mutex<Vec<(RawWebSocket, Instant)>>>>>>,
    max_per_key: usize,
    io_buf: usize,
    max_age: Duration,
}

impl ConnectionPool {
    pub fn new(max_per_key: usize, io_buf: usize, max_age_secs: u64) -> Self {
        Self {
            domains: Arc::new(RwLock::new(HashMap::new())),
            max_per_key,
            io_buf: crate::net::clamp_io_buf(io_buf),
            max_age: Duration::from_secs(max_age_secs.max(10)),
        }
    }

    fn queue_for(&self, domain: &str) -> Arc<Mutex<Vec<(RawWebSocket, Instant)>>> {
        // Fast path: read lock.
        {
            let map = self.domains.read().expect("pool map poisoned");
            if let Some(q) = map.get(domain) {
                return q.clone();
            }
        }
        // Slow path: create the queue once.
        let mut map = self.domains.write().expect("pool map poisoned");
        map.entry(domain.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
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
                let queues: Vec<(String, Arc<Mutex<Vec<(RawWebSocket, Instant)>>>)> = {
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

    pub async fn get(&self, domain: &str) -> Option<RawWebSocket> {
        let q = self.queue_for(domain);
        let mut list = q.lock().ok()?;
        let now = Instant::now();
        while let Some((ws, created)) = list.pop() {
            if now.duration_since(created) < self.max_age {
                return Some(ws);
            }
            debug!("Pool hit expired socket for {} (>max_age), closing", domain);
        }
        None
    }

    pub async fn put(&self, domain: String, ws: RawWebSocket) {
        let q = self.queue_for(&domain);
        let mut list = match q.lock() {
            Ok(l) => l,
            Err(_) => return,
        };
        if list.len() < self.max_per_key {
            list.push((ws, Instant::now()));
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
        if self.len(&domain) >= self.max_per_key {
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

    pub async fn warm_up(&self, domain: &str, path: &str, secure: bool, count: usize) {
        let domain_str = domain.to_string();
        let path_str = path.to_string();
        let max = self.max_per_key;
        let io_buf = self.io_buf;
        let this_domains = self.domains.clone();

        tokio::spawn(async move {
            for _ in 0..count.min(max) {
                match RawWebSocket::connect_with_buf(&domain_str, &domain_str, &path_str, secure, io_buf).await {
                    Ok(ws) => {
                        let q = {
                            let mut map = match this_domains.write() {
                                Ok(m) => m,
                                Err(_) => break,
                            };
                            map.entry(domain_str.clone())
                                .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
                                .clone()
                        };
                        let mut list = match q.lock() {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        if list.len() < max {
                            list.push((ws, Instant::now()));
                            info!("Warmed up connection to {}", domain_str);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Warm-up error for {}: {:?}", domain_str, e);
                        break;
                    }
                }
            }
        });
    }

    /// Pre-warm one connection per Telegram DC (non-media endpoints cover the
    /// common case; media subdomains fall back to on-demand connect).
    pub async fn warm_up_defaults(&self, secure: bool) {
        let domains: Arc<RwLock<HashMap<String, Arc<Mutex<Vec<(RawWebSocket, Instant)>>>>>> =
            self.domains.clone();
        let max = self.max_per_key;
        let io_buf = self.io_buf;
        tokio::spawn(async move {
            for dc in 1..=5 {
                let domain_str = format!("kws{}.web.telegram.org", dc);
                match RawWebSocket::connect_with_buf(&domain_str, &domain_str, "/apiws", secure, io_buf).await {
                    Ok(ws) => {
                        let q = {
                            let mut map = match domains.write() {
                                Ok(m) => m,
                                Err(_) => break,
                            };
                            map.entry(domain_str.clone())
                                .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
                                .clone()
                        };
                        let mut list = match q.lock() {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        if list.len() < max {
                            list.push((ws, Instant::now()));
                            info!("Pre-warmed connection in pool for {}", domain_str);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Warm-up error for {}: {:?}", domain_str, e);
                    }
                }
            }
        });
    }
}
