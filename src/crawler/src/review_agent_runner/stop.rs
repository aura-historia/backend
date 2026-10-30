//! The runner's stop. Its stop signal requests it, as `systemctl stop` sends SIGTERM to the runner
//! alone: from then on no agent process starts, and each running one is stopped as at its timeout,
//! first asked with SIGINT to end its turn with its result envelope, so its spend is recorded in
//! the ledger before the runner exits. The contract is in `docs/crawler/operations.md` (review
//! agent runner, stop and undo).

use super::process::LONGEST_CHILD_STOP;
use std::time::Duration;
use tokio::sync::watch;

/// How long the runner's stop may take before what still runs is dropped, which kills every agent
/// process group left: longer than stopping an agent process takes, so each one it stops is
/// recorded. The unit's `TimeoutStopSec` and `dev`'s wait for its runner leave room for it.
pub const STOP_DEADLINE: Duration = Duration::from_secs(20);

const _: () = assert!(STOP_DEADLINE.as_secs() > LONGEST_CHILD_STOP.as_secs());

/// The stop request every part of a run sees; clones share it.
#[derive(Debug, Clone, Default)]
pub struct RunnerStop(watch::Sender<bool>);

impl RunnerStop {
    pub fn request(&self) {
        self.0.send_replace(true);
    }

    pub fn is_requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once the stop is requested, at once when it was.
    pub async fn requested(&self) {
        let mut requests = self.0.subscribe();
        // `self` holds a sender, so the channel cannot close while this waits.
        let _ = requests.wait_for(|requested| *requested).await;
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
}
