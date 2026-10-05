use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use crate::raw_websocket::RawWebSocket;
use tracing::{debug, info};

pub const WS_POOL_MAX_AGE: Duration = Duration::from_secs(120);

pub struct ConnectionPool {
    pool: Arc<Mutex<HashMap<String, Vec<(RawWebSocket, Instant)>>>>,
    max_per_key: usize,
}

impl ConnectionPool {
    pub fn new(max_per_key: usize) -> Self {
        Self {
            pool: Arc::new(Mutex::new(HashMap::new())),
            max_per_key,
        }
    }

    pub fn start_cleanup_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let mut map = self.pool.lock().await;
                for (domain, list) in map.iter_mut() {
                    let before = list.len();
                    list.retain(|(_, created)| now.duration_since(*created) < WS_POOL_MAX_AGE);
                    let expired = before - list.len();
                    if expired > 0 {
                        debug!("Pool cleanup: removed {} expired sockets for {}", expired, domain);
                    }
                }
            }
        });
    }

    pub async fn get(&self, domain: &str) -> Option<RawWebSocket> {
        let mut map = self.pool.lock().await;
        if let Some(list) = map.get_mut(domain) {
            let now = Instant::now();
            while let Some((ws, created)) = list.pop() {
                if now.duration_since(created) < WS_POOL_MAX_AGE {
                    return Some(ws);
                }
                debug!("Pool hit expired socket for {} (>120s), closing", domain);
            }
        }
        None
    }

    pub async fn put(&self, domain: String, ws: RawWebSocket) {
        let mut map = self.pool.lock().await;
        let list = map.entry(domain).or_default();
        if list.len() < self.max_per_key {
            list.push((ws, Instant::now()));
        }
    }

    pub async fn warm_up(&self, domain: &str, path: &str, secure: bool, count: usize) {
        let pool = self.pool.clone();
        let domain_str = domain.to_string();
        let path_str = path.to_string();
        let max = self.max_per_key;

        tokio::spawn(async move {
            for _ in 0..count.min(max) {
                match RawWebSocket::connect(&domain_str, &domain_str, &path_str, secure).await {
                    Ok(ws) => {
                        let mut map = pool.lock().await;
                        let list = map.entry(domain_str.clone()).or_default();
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

    pub async fn warm_up_defaults(&self, secure: bool) {
        let pool = self.pool.clone();
        let max = self.max_per_key;
        tokio::spawn(async move {
            let defaults = ["kws2.web.telegram.org", "kws4.web.telegram.org"];
            for domain in defaults {
                let domain_str = domain.to_string();
                for _ in 0..max.min(2) {
                    match RawWebSocket::connect(&domain_str, &domain_str, "/apiws", secure).await {
                        Ok(ws) => {
                            let mut map = pool.lock().await;
                            let list = map.entry(domain_str.clone()).or_default();
                            if list.len() < max {
                                list.push((ws, Instant::now()));
                                info!("Pre-warmed connection in pool for {}", domain_str);
                            }
                        }
                        Err(e) => {
                            tracing::debug!("Warm-up error for {}: {:?}", domain_str, e);
                            break;
                        }
                    }
                }
            }
        });
    }
}
