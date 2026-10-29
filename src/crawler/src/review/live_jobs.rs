//! The review console's cap on its live jobs: the work that fetches a shop's pages live for a
//! review and evaluates them, namely live matrix refreshes, candidate evaluations and page
//! projections. One limit holds for the whole console, across reviews and actors, so the review
//! agents and open console tabs together never run more live jobs at once than
//! `CRAWLER_REVIEW_MAX_CONCURRENT_LIVE_JOBS` allows. A job that would go above it is refused at
//! once, never queued. The operator contract is in `docs/crawler/operations.md` (review service).

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

pub const MAX_CONCURRENT_LIVE_JOBS_ENV: &str = "CRAWLER_REVIEW_MAX_CONCURRENT_LIVE_JOBS";
pub const DEFAULT_MAX_CONCURRENT_LIVE_JOBS: NonZeroU32 = NonZeroU32::new(4).unwrap();
/// How long a refused client waits before it asks again: about the time one page fetch at a
/// shop's crawl delay takes.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The live jobs one review console runs at once. Clones share the same places.
#[derive(Clone)]
pub(crate) struct LiveJobLimit {
    places: Arc<Semaphore>,
    limit: NonZeroU32,
}

/// One running live job's place under the [`LiveJobLimit`]; dropping it frees the place. Whatever
/// runs the job's work holds it until that work ends: the spawned task of a background job, or the
/// closure of the blocking thread a job's parsing runs on, since a blocking thread outlives a
/// request that stops waiting for it.
#[must_use = "dropping the permit frees the live job's place at once"]
#[derive(Debug)]
pub(crate) struct LiveJobPermit {
    _place: OwnedSemaphorePermit,
}

/// The console runs as many live jobs as its limit allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveJobsBusy {
    pub(crate) limit: NonZeroU32,
    pub(crate) retry_after: Duration,
}

impl LiveJobLimit {
    pub(crate) fn new(limit: NonZeroU32) -> Self {
        let places = usize::try_from(limit.get())
            .unwrap_or(usize::MAX)
            .min(Semaphore::MAX_PERMITS);
        Self {
            places: Arc::new(Semaphore::new(places)),
            limit,
        }
    }

    /// How many live jobs run at once at most.
    pub(crate) fn limit(&self) -> NonZeroU32 {
        self.limit
    }

    /// Takes a place for a new live job, or refuses it while every place is taken.
    pub(crate) fn try_start(&self) -> Result<LiveJobPermit, LiveJobsBusy> {
        match Arc::clone(&self.places).try_acquire_owned() {
            Ok(place) => Ok(LiveJobPermit { _place: place }),
            // The semaphore is never closed, so a closed one only means no place either.
            Err(TryAcquireError::NoPermits | TryAcquireError::Closed) => Err(LiveJobsBusy {
                limit: self.limit,
                retry_after: RETRY_AFTER,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_refuse_a_job_above_the_limit_until_a_running_one_ends() {
        let limit = LiveJobLimit::new(NonZeroU32::new(2).unwrap());
        let shared = limit.clone();

        let first = limit.try_start();
        let second = shared.try_start();
        let refused = limit.try_start();

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(
            refused.err(),
            Some(LiveJobsBusy {
                limit: NonZeroU32::new(2).unwrap(),
                retry_after: RETRY_AFTER,
            })
        );
        drop(first);
        assert!(shared.try_start().is_ok());
    }
}
