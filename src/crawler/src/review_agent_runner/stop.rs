//! The runner's stop. Its stop signal requests it, as `systemctl stop` sends SIGTERM to the runner
//! alone: from then on no agent process starts, each running one may finish for
//! [`AGENT_DRAIN_DEADLINE`], and one still running then is stopped as at its timeout, first asked
//! with SIGINT to end its turn with its result envelope, so its spend is recorded in the ledger
//! before the runner exits. The contract is in `docs/crawler/operations.md` (review
//! agent runner, stop and undo).

use super::process::LONGEST_CHILD_STOP;
use std::time::Duration;
use tokio::sync::watch;

/// How long an agent process that runs when the runner's stop is requested may go on before it is
/// stopped. A stopped author or reviewer has spent its money for nothing: its staged work never
/// reaches the review. Ten minutes covers a normal run, which takes 30 to 100 seconds, and a
/// schema author, which takes several minutes; a hung process is stopped at its own timeout
/// otherwise, which is too long a stop to wait for.
pub const AGENT_DRAIN_DEADLINE: Duration = Duration::from_secs(10 * 60);

/// How long the runner's stop may take before what still runs is dropped, which kills every agent
/// process group left: the drain, then longer than stopping an agent process takes, so each one it
/// stops is recorded. The unit's `TimeoutStopSec` and `dev`'s wait for its runner leave room for
/// it.
pub const STOP_DEADLINE: Duration =
    Duration::from_secs(AGENT_DRAIN_DEADLINE.as_secs() + LONGEST_CHILD_STOP.as_secs() + 10);

/// The stop request every part of a run sees; clones share it.
#[derive(Debug, Clone)]
pub struct RunnerStop {
    requested: watch::Sender<bool>,
    drain: Duration,
}

impl Default for RunnerStop {
    fn default() -> Self {
        Self::with_drain(AGENT_DRAIN_DEADLINE)
    }
}

impl RunnerStop {
    /// A stop that lets running agent processes go on for `drain` after it is requested.
    pub fn with_drain(drain: Duration) -> Self {
        Self {
            requested: watch::Sender::new(false),
            drain,
        }
    }

    pub fn request(&self) {
        self.requested.send_replace(true);
    }

    pub fn is_requested(&self) -> bool {
        *self.requested.borrow()
    }

    /// Resolves once the stop is requested, at once when it was.
    pub async fn requested(&self) {
        let mut requests = self.requested.subscribe();
        // `self` holds a sender, so the channel cannot close while this waits.
        let _ = requests.wait_for(|requested| *requested).await;
    }

    /// Resolves when an agent process that runs must be stopped: the drain deadline after the
    /// stop was requested.
    pub async fn agent_drain_ended(&self) {
        self.requested().await;
        tokio::time::sleep(self.drain).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn should_tell_every_clone_once_the_stop_is_requested() {
        let stop = RunnerStop::default();
        let waiting = stop.clone();
        let waiter = tokio::spawn(async move { waiting.requested().await });
        tokio::task::yield_now().await;

        assert!(!stop.is_requested());
        assert!(!waiter.is_finished());
        stop.clone().request();

        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap_or_else(|_| panic!("the waiting clone did not see the stop"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(stop.is_requested());
        tokio::time::timeout(Duration::from_secs(5), stop.requested())
            .await
            .unwrap_or_else(|_| panic!("a stop already requested did not resolve at once"));
    }

    #[tokio::test(start_paused = true)]
    async fn should_end_the_agent_drain_only_after_the_drain_deadline_passed_since_the_stop() {
        let stop = RunnerStop::default();
        let draining = stop.clone();
        let drain = tokio::spawn(async move { draining.agent_drain_ended().await });

        tokio::time::sleep(AGENT_DRAIN_DEADLINE * 2).await;
        assert!(!drain.is_finished(), "the drain ran without a stop");

        stop.request();
        tokio::time::sleep(AGENT_DRAIN_DEADLINE - Duration::from_secs(1)).await;
        assert!(!drain.is_finished(), "the drain ended before its deadline");

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(drain.is_finished(), "the drain outlasted its deadline");
    }
}
