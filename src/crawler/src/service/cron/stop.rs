//! The switch that stops a [`CrawlerCronJob`](super::CrawlerCronJob) from starting work, for a
//! graceful process stop ([operations](../../../../../docs/crawler/operations.md#stopping-the-crawler)).

use std::time::Duration;
use tokio::sync::watch;

/// Asks the cron loops to stop starting work. Clones share one switch, and a requested stop
/// stays requested: no new pass, scraper domain batch or scraper URL starts, while work already
/// running finishes.
#[derive(Clone)]
pub struct CronStop(watch::Sender<bool>);

impl Default for CronStop {
    fn default() -> Self {
        Self(watch::Sender::new(false))
    }
}

impl CronStop {
    pub fn request(&self) {
        self.0.send_replace(true);
    }

    pub(crate) fn is_requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Waits for `interval`, ending early when a stop is requested meanwhile.
    pub(super) async fn sleep(&self, interval: Duration) {
        let mut stop = self.0.subscribe();
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            // `self` keeps the sender alive, so the wait ends only on a request.
            _ = stop.wait_for(|requested| *requested) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn should_end_sleep_early_when_a_clone_requests_the_stop() {
        let stop = CronStop::default();
        let requester = stop.clone();
        let started = tokio::time::Instant::now();

        tokio::join!(stop.sleep(Duration::from_secs(3600)), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            requester.request();
        });

        assert!(stop.is_requested());
        assert_eq!(started.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn should_not_sleep_once_the_stop_was_requested() {
        let stop = CronStop::default();
        stop.request();
        let started = tokio::time::Instant::now();

        stop.sleep(Duration::from_secs(3600)).await;

        assert_eq!(started.elapsed(), Duration::ZERO);
    }
}
