use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default, Debug)]
pub struct Stats {
    pub connections_total: AtomicU64,
    pub connections_active: AtomicU64,
    pub connections_ws: AtomicU64,
    pub connections_cfproxy: AtomicU64,
    pub connections_tcp: AtomicU64,
    pub connections_bad: AtomicU64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub connections_total: u64,
    pub connections_active: u64,
    pub connections_ws: u64,
    pub connections_cfproxy: u64,
    pub connections_tcp: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub speed_up_kbps: f64,
    pub speed_down_kbps: f64,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_total(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_active(&self) {
        self.connections_active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn add_up(&self, n: usize) {
        self.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn add_down(&self, n: usize) {
        self.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self, speed_up: f64, speed_down: f64) -> TelemetrySnapshot {
        TelemetrySnapshot {
            connections_total: self.connections_total.load(Ordering::Relaxed),
            connections_active: self.connections_active.load(Ordering::Relaxed),
            connections_ws: self.connections_ws.load(Ordering::Relaxed),
            connections_cfproxy: self.connections_cfproxy.load(Ordering::Relaxed),
            connections_tcp: self.connections_tcp.load(Ordering::Relaxed),
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
            speed_up_kbps: speed_up,
            speed_down_kbps: speed_down,
        }
    }
}
