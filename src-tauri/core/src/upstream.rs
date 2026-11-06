//! Bounded upstream selection: stop losers as soon as one usable route wins.
use crate::{balancer::Balancer, config::ProxyConfig, raw_websocket::RawWebSocket};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::task::JoinSet;

#[derive(Default)]
pub(crate) struct BackgroundTasks(pub Vec<tokio::task::JoinHandle<()>>);
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// One budget for the entire list, rather than one timeout per dead endpoint.
pub(crate) async fn race<T, F>(
    attempts: impl IntoIterator<Item = F>,
    width: usize,
    budget: Duration,
) -> Option<T>
where
    T: Send + 'static,
    F: Future<Output = Option<T>> + Send + 'static,
{
    let mut attempts = attempts.into_iter();
    let mut tasks = JoinSet::new();
    for attempt in attempts.by_ref().take(width.max(1)) {
        tasks.spawn(attempt);
    }
    let deadline = tokio::time::sleep(budget);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = &mut deadline => return None,
            result = tasks.join_next() => {
                match result {
                    Some(Ok(Some(winner))) => return Some(winner),
                    None => return None,
                    _ => if let Some(attempt) = attempts.next() { tasks.spawn(attempt); },
                }
            }
        }
    }
    // Dropping JoinSet aborts every losing attempt, including on cancellation.
}

pub(crate) struct WsTarget {
    pub key: String,
    pub host: String,
    pub path: String,
}

pub(crate) async fn race_ws(
    targets: Vec<WsTarget>,
    cfg: &ProxyConfig,
    init: [u8; 64],
    balancer: Arc<Balancer>,
) -> Option<(String, RawWebSocket)> {
    let timeout = Duration::from_secs(cfg.connect_timeout_secs.clamp(2, 120));
    let secure = !cfg.disable_secure;
    let io_buf = cfg.buffer_size;
    let attempts = targets.into_iter().map(|target| {
        let balancer = balancer.clone();
        async move {
            let start = std::time::Instant::now();
            let result = tokio::time::timeout(timeout, async {
                let mut ws = RawWebSocket::connect_with_buf(
                    &target.host,
                    &target.host,
                    &target.path,
                    secure,
                    io_buf,
                )
                .await?;
                ws.send(&init).await?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(ws)
            })
            .await;
            match result {
                Ok(Ok(ws)) => {
                    balancer
                        .record_success(&target.key, start.elapsed().as_micros())
                        .await;
                    Some((target.host, ws))
                }
                error => {
                    let reason = match error {
                        Ok(Err(error)) => error.to_string(),
                        Err(error) => error.to_string(),
                        Ok(Ok(_)) => unreachable!(),
                    };
                    tracing::debug!(
                        "Upstream {} failed after {:?}: {}",
                        target.host,
                        start.elapsed(),
                        reason
                    );
                    balancer.record_failure(&target.key).await;
                    None
                }
            }
        }
    });
    race(attempts, 4, timeout * 2).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn bridge_shutdown_cancels_maintenance() {
        let task = tokio::spawn(std::future::pending::<()>());
        let aborted = task.abort_handle();
        let group = BackgroundTasks(vec![task]);
        drop(group);
        tokio::task::yield_now().await;
        assert!(aborted.is_finished());
    }

    #[tokio::test]
    async fn winner_does_not_wait_for_blackholed_peer() {
        let active = Arc::new(AtomicUsize::new(0));
        let attempts = [false, true].map(|works| {
            let active = active.clone();
            async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = Active(active);
                if works {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Some(7)
                } else {
                    std::future::pending().await
                }
            }
        });
        assert_eq!(race(attempts, 2, Duration::from_secs(1)).await, Some(7));
        tokio::task::yield_now().await;
        assert_eq!(
            active.load(Ordering::SeqCst),
            0,
            "losing connections must be cancelled"
        );
    }

    #[tokio::test]
    async fn dead_list_has_one_total_budget() {
        let attempts = (0..20).map(|_| std::future::pending::<Option<()>>());
        assert!(
            tokio::time::timeout(
                Duration::from_secs(1),
                race(attempts, 4, Duration::from_millis(20))
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn failure_opens_a_slot_for_next_candidate() {
        let attempts = (0..8).map(|i| async move { (i == 7).then_some(i) });
        assert_eq!(race(attempts, 2, Duration::from_secs(1)).await, Some(7));
    }
}
