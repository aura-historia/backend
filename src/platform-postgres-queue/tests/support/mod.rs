//! Scopes, queues and assertions shared by the queue's integration tests.

#![allow(dead_code)]

use platform_postgres_queue::{
    ClaimedJob, Disposition, EnqueueOutcome, ErrorClass, GroupKey, JobId, JobQueue, JobScope,
    NewJob, QueueOrdering, ScopeName, Settlement, enqueue,
};
use sqlx::PgPool;
use std::time::Duration;
use test_api::Postgres;

pub const DATABASE: Postgres = Postgres::new("migrations");

/// Long enough that no lease expires during a test that does not wait for it.
pub const LONG: Duration = Duration::from_secs(300);
pub const SHORT: Duration = Duration::from_millis(300);

pub fn scope(
    name: &str,
    ordering: QueueOrdering,
    lease: Duration,
    max_attempts: u32,
    retry_delay: Duration,
) -> JobScope {
    JobScope::new(
        ScopeName::new(name).unwrap(),
        ordering,
        lease,
        max_attempts,
        retry_delay,
    )
    .unwrap()
}

pub fn standard(name: &str) -> JobScope {
    scope(name, QueueOrdering::Standard, LONG, 5, Duration::ZERO)
}

pub fn fifo(name: &str) -> JobScope {
    scope(name, QueueOrdering::Fifo, LONG, 5, Duration::ZERO)
}

pub fn class(name: &str) -> ErrorClass {
    ErrorClass::new(name).unwrap()
}

pub async fn push(pool: &PgPool, scope: &JobScope, group: Option<&str>, payload: &str) -> JobId {
    let job = NewJob::new(scope, group.map(|g| GroupKey::new(g).unwrap()), payload).unwrap();
    match enqueue(pool, &job).await.unwrap() {
        EnqueueOutcome::Enqueued(id) => id,
        EnqueueOutcome::Deduplicated => panic!("unexpected deduplication"),
    }
}

pub async fn claim_one(queue: &JobQueue) -> Option<ClaimedJob> {
    let mut jobs = queue.claim(1).await.unwrap();
    assert!(jobs.len() <= 1);
    jobs.pop()
}

pub async fn claim_ids(queue: &JobQueue, limit: u16) -> Vec<JobId> {
    let mut ids: Vec<_> = queue
        .claim(limit)
        .await
        .unwrap()
        .iter()
        .map(ClaimedJob::job_id)
        .collect();
    ids.sort();
    ids
}

pub async fn ack(queue: &JobQueue, job: &ClaimedJob) {
    assert_eq!(
        Settlement::Acknowledged,
        queue
            .settle(job.lease(), Disposition::Complete)
            .await
            .unwrap()
    );
}

pub fn sorted(mut ids: Vec<JobId>) -> Vec<JobId> {
    ids.sort();
    ids
}

/// The custody row of a job: payload, attempts, reason and last error class.
pub async fn custody_row(pool: &PgPool, id: JobId) -> (String, i32, String, Option<String>) {
    sqlx::query_as(
        "SELECT payload, attempts, reason, last_error_class FROM job_queue_failures \
         WHERE job_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The last error class of a queued job.
pub async fn last_error_class(pool: &PgPool, id: JobId) -> Option<String> {
    sqlx::query_scalar("SELECT last_error_class FROM job_queue_jobs WHERE job_id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}
