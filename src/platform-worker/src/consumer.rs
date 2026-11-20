//! The claim loop of one scope.

use crate::{error_class::error_class, job::Job};
use async_trait::async_trait;
use platform_postgres::PostgresFailure;
use platform_postgres_queue::{ClaimedJob, Disposition, JobQueue, JobScope, Settlement};
use std::{num::NonZeroU16, sync::Arc, time::Duration};
use tokio::{sync::Notify, task::JoinSet};
use tokio_util::sync::CancellationToken;

/// Waits after a failed claim before claiming again, so an unavailable database is not hammered.
const CLAIM_FAILURE_BACKOFF: Duration = Duration::from_secs(1);

/// Turns one claimed job into its disposition. It owns the job's semantics: decoding, the use
/// case, and which outcomes are complete, retried or parked. It must tolerate redelivery.
#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    async fn handle(&self, job: &Job) -> Disposition;
}

/// How one scope's consumer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsumerConfig {
    concurrency: NonZeroU16,
    job_timeout: Duration,
    poll_interval: Duration,
    shutdown_grace: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsumerConfigError {
    #[error("a job timeout must be positive and shorter than its scope's lease")]
    JobTimeoutNotWithinLease,
    #[error("a poll interval must be positive")]
    ZeroPollInterval,
}

impl ConsumerConfig {
    /// - `concurrency`: jobs of the scope this process runs at once.
    /// - `job_timeout`: the most one attempt may run; it replaces the Lambda invocation budget.
    ///   It must end before the lease, so a slow attempt is retried by this worker rather than
    ///   redelivered to another while it still runs.
    /// - `poll_interval`: the longest an idle consumer waits without a notification.
    /// - `shutdown_grace`: how long in-flight jobs may finish after shutdown starts before they
    ///   are abandoned and their leases released.
    pub fn new(
        scope: &JobScope,
        concurrency: NonZeroU16,
        job_timeout: Duration,
        poll_interval: Duration,
        shutdown_grace: Duration,
    ) -> Result<Self, ConsumerConfigError> {
        if job_timeout.is_zero() || job_timeout >= scope.lease() {
            return Err(ConsumerConfigError::JobTimeoutNotWithinLease);
        }
        if poll_interval.is_zero() {
            return Err(ConsumerConfigError::ZeroPollInterval);
        }
        Ok(Self {
            concurrency,
            job_timeout,
            poll_interval,
            shutdown_grace,
        })
    }

    pub fn concurrency(&self) -> NonZeroU16 {
        self.concurrency
    }

    pub fn job_timeout(&self) -> Duration {
        self.job_timeout
    }
}

/// One scope's consumer.
pub struct Consumer {
    queue: JobQueue,
    handler: Arc<dyn JobHandler>,
    config: ConsumerConfig,
}

impl Consumer {
    pub fn new(queue: JobQueue, handler: Arc<dyn JobHandler>, config: ConsumerConfig) -> Self {
        Self {
            queue,
            handler,
            config,
        }
    }

    pub fn scope(&self) -> &JobScope {
        self.queue.scope()
    }

    /// Claims and runs jobs until `shutdown`. It then stops claiming, lets in-flight jobs finish
    /// within the shutdown grace, and releases the leases of the rest, so they are claimable at
    /// once instead of after their lease.
    pub async fn run(self, wakeup: Arc<Notify>, shutdown: CancellationToken) {
        let abandon = CancellationToken::new();
        let mut in_flight = JoinSet::new();
        let concurrency = usize::from(self.config.concurrency.get());
        while !shutdown.is_cancelled() {
            while in_flight.try_join_next().is_some() {}
            let free = concurrency - in_flight.len();
            if free > 0 {
                let limit = u16::try_from(free).unwrap_or(u16::MAX);
                match self.queue.claim(limit).await {
                    Ok(jobs) => {
                        let saturated = jobs.len() == free;
                        for job in jobs {
                            in_flight.spawn(attempt(
                                self.queue.clone(),
                                Arc::clone(&self.handler),
                                job,
                                self.config.job_timeout,
                                abandon.clone(),
                            ));
                        }
                        if saturated {
                            // More jobs may be waiting; claim again once a slot frees.
                            wait_for_slot(&mut in_flight, &shutdown).await;
                            continue;
                        }
                    }
                    Err(error) => {
                        let failure = PostgresFailure::find(&error);
                        let sqlstate = failure.and_then(|f| f.sqlstate());
                        tracing::warn!(
                            event = "worker.claim.failed",
                            job_scope = self.scope().name().as_str(),
                            error_category = failure.map_or("unclassified", |f| f.kind()),
                            sqlstate = sqlstate.as_ref().map(|s| s.as_str()),
                            "Claim failed; retrying after a backoff"
                        );
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(CLAIM_FAILURE_BACKOFF) => continue,
                        }
                    }
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                Some(_) = in_flight.join_next(), if !in_flight.is_empty() => {}
                () = wakeup.notified() => {}
                () = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
        self.drain(in_flight, abandon).await;
    }

    async fn drain(&self, mut in_flight: JoinSet<()>, abandon: CancellationToken) {
        if in_flight.is_empty() {
            return;
        }
        let finished = tokio::time::timeout(self.config.shutdown_grace, async {
            while in_flight.join_next().await.is_some() {}
        })
        .await;
        if finished.is_err() {
            tracing::info!(
                event = "worker.shutdown.releasing",
                job_scope = self.scope().name().as_str(),
                in_flight = in_flight.len(),
                "Shutdown grace elapsed; releasing unfinished jobs"
            );
            abandon.cancel();
            while in_flight.join_next().await.is_some() {}
        }
    }
}

async fn wait_for_slot(in_flight: &mut JoinSet<()>, shutdown: &CancellationToken) {
    tokio::select! {
        () = shutdown.cancelled() => {}
        _ = in_flight.join_next() => {}
    }
}

/// Runs one attempt in its own task, so a panic is contained, bounds it by the job timeout and
/// settles it. Once `abandon` fires the attempt is cancelled and the job released.
async fn attempt(
    queue: JobQueue,
    handler: Arc<dyn JobHandler>,
    job: ClaimedJob,
    timeout: Duration,
    abandon: CancellationToken,
) {
    let lease = *job.lease();
    let job = Job::from(job);
    let mut handle = tokio::spawn(async move { handler.handle(&job).await });
    let disposition = tokio::select! {
        () = abandon.cancelled() => {
            handle.abort();
            let _ = handle.await;
            log_settlement(&queue, "released", queue.release(&lease).await);
            return;
        }
        outcome = tokio::time::timeout(timeout, &mut handle) => match outcome {
            Ok(Ok(disposition)) => disposition,
            Ok(Err(error)) if error.is_panic() => {
                tracing::warn!(
                    event = "worker.job.panicked",
                    job_scope = queue.scope().name().as_str(),
                    job_id = %lease.job_id(),
                    "Job handler panicked; the job is retried"
                );
                Disposition::Retry(error_class("panicked"))
            }
            Ok(Err(_)) => Disposition::Retry(error_class("cancelled")),
            Err(_) => {
                handle.abort();
                // The cancelled attempt stops at its next await point; its connection is
                // returned before the slot is reused.
                let _ = handle.await;
                tracing::warn!(
                    event = "worker.job.timed_out",
                    job_scope = queue.scope().name().as_str(),
                    job_id = %lease.job_id(),
                    timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    "Job exceeded its timeout; the job is retried"
                );
                Disposition::Retry(error_class("timed_out"))
            }
        },
    };
    let label = match &disposition {
        Disposition::Complete => "complete",
        Disposition::Retry(_) => "retry",
        Disposition::Park(_) => "park",
    };
    log_settlement(&queue, label, queue.settle(&lease, disposition).await);
}

fn log_settlement(
    queue: &JobQueue,
    disposition: &'static str,
    settlement: Result<Settlement, platform_postgres_queue::JobQueueError>,
) {
    let scope = queue.scope().name().as_str();
    match settlement {
        Ok(Settlement::Acknowledged | Settlement::Released | Settlement::MovedToCustody(_)) => {
            tracing::debug!(event = "worker.job.settled", job_scope = scope, disposition);
        }
        Ok(Settlement::StaleLease) => tracing::warn!(
            event = "worker.job.stale_lease",
            job_scope = scope,
            disposition,
            "Job lease was superseded before settlement; its new holder settles it"
        ),
        Err(error) => {
            let failure = PostgresFailure::find(&error);
            let sqlstate = failure.and_then(|f| f.sqlstate());
            tracing::warn!(
                event = "worker.job.settlement_failed",
                job_scope = scope,
                disposition,
                error_category = failure.map_or("unclassified", |f| f.kind()),
                sqlstate = sqlstate.as_ref().map(|s| s.as_str()),
                "Job settlement failed; the job is redelivered when its lease expires"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_postgres_queue::{QueueOrdering, ScopeName};

    fn scope(lease: Duration) -> JobScope {
        JobScope::new(
            ScopeName::new("scope").unwrap(),
            QueueOrdering::Standard,
            lease,
            5,
            lease,
        )
        .unwrap()
    }

    #[test]
    fn should_require_the_job_timeout_to_end_within_the_lease() {
        let concurrency = NonZeroU16::new(2).unwrap();
        let second = Duration::from_secs(1);
        let lease = scope(Duration::from_secs(30));

        assert!(ConsumerConfig::new(&lease, concurrency, 29 * second, second, second).is_ok());
        for timeout in [Duration::ZERO, 30 * second, 31 * second] {
            assert_eq!(
                Err(ConsumerConfigError::JobTimeoutNotWithinLease),
                ConsumerConfig::new(&lease, concurrency, timeout, second, second)
            );
        }
        assert_eq!(
            Err(ConsumerConfigError::ZeroPollInterval),
            ConsumerConfig::new(&lease, concurrency, second, Duration::ZERO, second)
        );
    }
}
