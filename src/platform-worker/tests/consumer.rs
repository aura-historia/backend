//! A scope's consumer against real PostgreSQL: settlement, panic and timeout containment, and a
//! graceful shutdown that either completes an in-flight job or releases its lease.

use async_trait::async_trait;
use platform_postgres_queue::{
    ClaimedJob, Disposition, EnqueueOutcome, ErrorClass, JobId, JobQueue, JobScope, NewJob,
    QueueOrdering, ScopeName, enqueue,
};
use platform_worker::{Consumer, ConsumerConfig, Job, JobHandler, ScopeWakeups};
use sqlx::PgPool;
use std::{num::NonZeroU16, sync::Arc, time::Duration};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

const DATABASE: Postgres = Postgres::new("migrations");
const LEASE: Duration = Duration::from_secs(300);
const WAIT: Duration = Duration::from_secs(20);

fn scope(name: &str, retry_delay: Duration) -> JobScope {
    JobScope::new(
        ScopeName::new(name).unwrap(),
        QueueOrdering::Standard,
        LEASE,
        5,
        retry_delay,
    )
    .unwrap()
}

fn config(scope: &JobScope, job_timeout: Duration, grace: Duration) -> ConsumerConfig {
    ConsumerConfig::new(
        scope,
        NonZeroU16::new(4).unwrap(),
        job_timeout,
        Duration::from_millis(100),
        grace,
    )
    .unwrap()
}

async fn push(pool: &PgPool, scope: &JobScope, payload: &str) -> JobId {
    match enqueue(pool, &NewJob::new(scope, None, payload).unwrap())
        .await
        .unwrap()
    {
        EnqueueOutcome::Enqueued(id) => id,
        EnqueueOutcome::Deduplicated => panic!("unexpected deduplication"),
    }
}

/// The job's attempts, lease state and last error class, or `None` once it is acknowledged.
async fn job_row(pool: &PgPool, id: JobId) -> Option<(i32, bool, Option<String>)> {
    sqlx::query_as(
        "SELECT attempts, lease_token IS NOT NULL, last_error_class FROM job_queue_jobs \
         WHERE job_id = $1",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn eventually<F, Fut>(mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("condition reached in time");
}

/// Completes `quick` and `fail` jobs at once; `slow` jobs announce their start and finish only
/// when released; `panic` jobs panic; `hang` jobs never finish.
struct ScriptedHandler {
    started: mpsc::UnboundedSender<String>,
    finish_slow: Arc<Notify>,
}

#[async_trait]
impl JobHandler for ScriptedHandler {
    async fn handle(&self, job: &Job) -> Disposition {
        let _ = self.started.send(job.payload().to_owned());
        match job.payload() {
            "slow" => {
                self.finish_slow.notified().await;
                Disposition::Complete
            }
            "panic" => panic!("scripted handler panic"),
            "hang" => std::future::pending().await,
            "fail" => Disposition::Retry(ErrorClass::new("scripted_failure").unwrap()),
            _ => Disposition::Complete,
        }
    }
}

struct Running {
    started: mpsc::UnboundedReceiver<String>,
    finish_slow: Arc<Notify>,
    shutdown: CancellationToken,
    consumer: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn wait_started(&mut self, payload: &str) {
        let started = tokio::time::timeout(WAIT, async {
            loop {
                if self.started.recv().await.as_deref() == Some(payload) {
                    return;
                }
            }
        })
        .await;
        assert!(started.is_ok(), "{payload} job started");
    }

    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(WAIT, self.consumer)
            .await
            .expect("consumer stops")
            .unwrap();
    }
}

fn start(pool: &PgPool, scope: &JobScope, config: ConsumerConfig) -> Running {
    let (sender, started) = mpsc::unbounded_channel();
    let finish_slow = Arc::new(Notify::new());
    let handler = Arc::new(ScriptedHandler {
        started: sender,
        finish_slow: Arc::clone(&finish_slow),
    });
    let mut wakeups = ScopeWakeups::default();
    let wakeup = wakeups.register(scope.name());
    let shutdown = CancellationToken::new();
    tokio::spawn(wakeups.listen(pool.clone(), shutdown.clone()));
    let consumer = Consumer::new(JobQueue::new(pool.clone(), scope.clone()), handler, config);
    let consumer = tokio::spawn(consumer.run(wakeup, shutdown.clone()));
    Running {
        started,
        finish_slow,
        shutdown,
        consumer,
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_acknowledge_completed_jobs_and_retry_failed_ones() {
    let pool = get_postgres_client().await;
    let scope = scope("settle", LEASE);
    let running = start(&pool, &scope, config(&scope, Duration::from_secs(5), WAIT));

    let quick = push(&pool, &scope, "quick").await;
    let failed = push(&pool, &scope, "fail").await;

    eventually(|| async { job_row(&pool, quick).await.is_none() }).await;
    eventually(|| async {
        job_row(&pool, failed).await == Some((1, false, Some("scripted_failure".into())))
    })
    .await;
    running.stop().await;
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_retry_a_job_whose_handler_panics_or_exceeds_its_timeout() {
    let pool = get_postgres_client().await;
    let scope = scope("contained", LEASE);
    let mut running = start(
        &pool,
        &scope,
        config(&scope, Duration::from_millis(300), WAIT),
    );

    let panicked = push(&pool, &scope, "panic").await;
    let hung = push(&pool, &scope, "hang").await;
    running.wait_started("hang").await;

    eventually(|| async {
        job_row(&pool, panicked).await == Some((1, false, Some("panicked".into())))
    })
    .await;
    eventually(|| async {
        job_row(&pool, hung).await == Some((1, false, Some("timed_out".into())))
    })
    .await;
    let quick = push(&pool, &scope, "quick").await;
    eventually(|| async { job_row(&pool, quick).await.is_none() }).await;
    running.stop().await;
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_let_an_in_flight_job_complete_within_the_shutdown_grace() {
    let pool = get_postgres_client().await;
    let scope = scope("graceful", LEASE);
    let mut running = start(&pool, &scope, config(&scope, Duration::from_secs(60), WAIT));
    let slow = push(&pool, &scope, "slow").await;
    running.wait_started("slow").await;

    running.shutdown.cancel();
    let late = push(&pool, &scope, "quick").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.finish_slow.notify_one();
    running.stop().await;

    assert_eq!(None, job_row(&pool, slow).await, "in-flight job completed");
    assert_eq!(
        Some((0, false, None)),
        job_row(&pool, late).await,
        "nothing is claimed after shutdown starts"
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_release_the_lease_of_a_job_still_running_when_the_grace_ends() {
    let pool = get_postgres_client().await;
    let scope = scope("released", LEASE);
    let mut running = start(
        &pool,
        &scope,
        config(&scope, Duration::from_secs(60), Duration::from_millis(200)),
    );
    let slow = push(&pool, &scope, "slow").await;
    running.wait_started("slow").await;
    assert_eq!(Some((1, true, None)), job_row(&pool, slow).await);

    running.stop().await;

    assert_eq!(
        Some((1, false, Some("released".into()))),
        job_row(&pool, slow).await,
        "the lease is released, not left to expire"
    );
    let reclaimed = JobQueue::new(pool.clone(), scope).claim(1).await.unwrap();
    assert_eq!(
        vec![slow],
        reclaimed.iter().map(ClaimedJob::job_id).collect::<Vec<_>>(),
        "another worker claims it at once"
    );
}
