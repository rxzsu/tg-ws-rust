use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// State of one Telegram DC's upstream route, as last observed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DcRoute {
    pub dc: i32,
    /// `ok` | `no_front` | `failed` | `timeout` | `no_cf`
    pub state: String,
    /// Human-readable reason, already safe for the UI.
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub connections_total: AtomicU64,
    pub connections_active: AtomicU64,
    pub connections_ws: AtomicU64,
    pub connections_quic: AtomicU64,
    pub connections_h3: AtomicU64,
    pub connections_cfproxy: AtomicU64,
    pub connections_tcp: AtomicU64,
    pub connections_bad: AtomicU64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
    /// Epoch millis of the last proxy start (0 = never started).
    started_at_ms: AtomicU64,
    /// Last user-visible failure, surfaced in the dashboard.
    last_error: RwLock<String>,
    /// Per-DC route state, newest first, surfaced in the dashboard.
    dc_routes: RwLock<Vec<DcRoute>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub connections_total: u64,
    pub connections_active: u64,
    pub connections_ws: u64,
    pub connections_quic: u64,
    pub connections_h3: u64,
    pub connections_cfproxy: u64,
    pub connections_tcp: u64,
    pub connections_bad: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub speed_up_kbps: f64,
    pub speed_down_kbps: f64,
    /// Uptime, in seconds since the current run started (0 when stopped).
    pub uptime_secs: u64,
    /// Last connection error, empty when there is nothing to report.
    pub last_error: String,
    /// Last observed upstream route per DC.
    pub dc_routes: Vec<DcRoute>,
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
        // Saturating: a stop (or a bug elsewhere) must never wrap this to
        // u64::MAX and make the dashboard show 18 quintillion connections.
        let _ = self
            .connections_active
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }

    pub fn add_up(&self, n: usize) {
        self.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn add_down(&self, n: usize) {
        self.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// Mark the start of a proxy run: resets uptime and the last error.
    pub fn mark_started(&self) {
        self.started_at_ms.store(now_ms(), Ordering::Relaxed);
        self.clear_last_error();
    }

    /// Stop the uptime clock (called when the bridge is stopped).
    pub fn mark_stopped(&self) {
        self.started_at_ms.store(0, Ordering::Relaxed);
    }

    /// Remember the last failure so the UI can show why nothing connects.
    pub fn set_last_error(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.last_error.write() {
            *guard = message.into();
        }
    }

    /// Record how the upstream route for `dc` ended up (upsert, newest first).
    pub fn record_dc_route(&self, dc: i32, state: &str, detail: impl Into<String>) {
        let route = DcRoute {
            dc,
            state: state.to_string(),
            detail: detail.into(),
        };
        if let Ok(mut guard) = self.dc_routes.write() {
            guard.retain(|r| r.dc != dc);
            guard.insert(0, route);
            // Only the observed DCs matter; keep the list bounded anyway.
            guard.truncate(12);
        }
    }

    pub fn clear_last_error(&self) {
        if let Ok(mut guard) = self.last_error.write() {
            guard.clear();
        }
    }

    pub fn snapshot(&self, speed_up: f64, speed_down: f64) -> TelemetrySnapshot {
        let started = self.started_at_ms.load(Ordering::Relaxed);
        let uptime_secs = if started == 0 {
            0
        } else {
            now_ms().saturating_sub(started) / 1000
        };
        TelemetrySnapshot {
            connections_total: self.connections_total.load(Ordering::Relaxed),
            connections_active: self.connections_active.load(Ordering::Relaxed),
            connections_ws: self.connections_ws.load(Ordering::Relaxed),
            connections_quic: self.connections_quic.load(Ordering::Relaxed),
            connections_h3: self.connections_h3.load(Ordering::Relaxed),
            connections_cfproxy: self.connections_cfproxy.load(Ordering::Relaxed),
            connections_tcp: self.connections_tcp.load(Ordering::Relaxed),
            connections_bad: self.connections_bad.load(Ordering::Relaxed),
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
            speed_up_kbps: speed_up,
            speed_down_kbps: speed_down,
            uptime_secs,
            last_error: self
                .last_error
                .read()
                .map(|g| g.clone())
                .unwrap_or_default(),
            dc_routes: self.dc_routes.read().map(|g| g.clone()).unwrap_or_default(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_is_zero_before_start_and_grows_after() {
        let stats = Stats::new();
        assert_eq!(stats.snapshot(0.0, 0.0).uptime_secs, 0);

        stats.mark_started();
        let snap = stats.snapshot(0.0, 0.0);
        assert_eq!(snap.uptime_secs, 0, "just started: less than a second");

        stats.mark_stopped();
        assert_eq!(stats.snapshot(0.0, 0.0).uptime_secs, 0);
    }

    #[test]
    fn last_error_is_reported_and_cleared_on_start() {
        let stats = Stats::new();
        stats.set_last_error("boom");
        assert_eq!(stats.snapshot(0.0, 0.0).last_error, "boom");

        stats.mark_started();
        assert!(stats.snapshot(0.0, 0.0).last_error.is_empty());
    }

    #[test]
    fn dc_routes_are_upserted_newest_first() {
        let stats = Stats::new();
        stats.record_dc_route(1, "no_front", "no /apiws front");
        stats.record_dc_route(4, "ok", "101 OK");

        let routes = stats.snapshot(0.0, 0.0).dc_routes;
        assert_eq!(routes.len(), 2, "one entry per DC, not duplicates");
        assert_eq!(routes[0].dc, 4, "newest DC comes first");

        // A new observation for the same DC replaces the old one in place.
        stats.record_dc_route(1, "timeout", "still blackholed");
        let routes = stats.snapshot(0.0, 0.0).dc_routes;
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].state, "timeout");
    }

    #[test]
    fn dec_active_never_underflows() {
        let stats = Stats::new();
        stats.dec_active();
        assert_eq!(stats.connections_active.load(Ordering::Relaxed), 0);
        stats.inc_active();
        stats.dec_active();
        assert_eq!(stats.connections_active.load(Ordering::Relaxed), 0);
    }
}
