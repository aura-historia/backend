//! Manual throughput measurement, ignored by default:
//!
//! ```sh
//! cargo test --locked -p platform-postgres-queue --release --test throughput -- --ignored --nocapture
//! ```
//!
//! Producers enqueue one job per statement; workers claim batches and acknowledge each job on
//! its own, as a worker settles jobs independently.

mod support;

use futures::future::join_all;
use platform_postgres_queue::{JobQueue, JobScope, NewJob, enqueue};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Instant;
use support::{DATABASE, ack, fifo, standard};
use test_api::{IntegrationTestService, TestContext, aura_integration_test};

const JOBS: usize = 40_000;
const CONCURRENCY: usize = 16;
const CLAIM_BATCH: u16 = 32;

async fn pool(context: &TestContext) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(u32::try_from(CONCURRENCY * 2).unwrap())
        .connect_with(context.postgres_connect_options().unwrap())
        .await
        .unwrap();
    context.register_postgres_pool(pool.clone()).unwrap();
    pool
}

async fn measure(pool: PgPool, scope: JobScope, groups: Option<usize>) {
    let started = Instant::now();
    let producers = (0..CONCURRENCY).map(|producer| {
        let (pool, scope) = (pool.clone(), scope.clone());
        async move {
            for index in (producer..JOBS).step_by(CONCURRENCY) {
                let group = groups.map(|groups| {
                    platform_postgres_queue::GroupKey::new(format!("g{}", index % groups)).unwrap()
                });
                let job = NewJob::new(&scope, group, format!("{{\"n\":{index}}}")).unwrap();
                enqueue(&pool, &job).await.unwrap();
            }
        }
    });
    join_all(producers).await;
    let enqueue_rate = JOBS as f64 / started.elapsed().as_secs_f64();

    let queue = JobQueue::new(pool, scope.clone());
    let started = Instant::now();
    let workers = (0..CONCURRENCY).map(|_| {
        let queue = queue.clone();
        async move {
            let mut processed = 0;
            let mut idle_rounds = 0;
            while idle_rounds < 3 {
                let jobs = queue.claim(CLAIM_BATCH).await.unwrap();
                if jobs.is_empty() {
                    idle_rounds += 1;
                    continue;
                }
                idle_rounds = 0;
                join_all(jobs.iter().map(|job| ack(&queue, job))).await;
                processed += jobs.len();
            }
            processed
        }
    });
    let processed: usize = join_all(workers).await.into_iter().sum();
    let claim_ack_rate = processed as f64 / started.elapsed().as_secs_f64();

    assert_eq!(JOBS, processed);
    println!(
        "{}: enqueue {enqueue_rate:.0} jobs/s, claim+ack {claim_ack_rate:.0} jobs/s \
         ({JOBS} jobs, {CONCURRENCY} producers/workers, claim batch {CLAIM_BATCH})",
        scope.name()
    );
}

#[ignore = "manual throughput measurement"]
#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_measure_standard_scope_throughput(context: TestContext) {
    measure(pool(&context).await, standard("throughput-standard"), None).await;
}

#[ignore = "manual throughput measurement"]
#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_measure_fifo_scope_throughput(context: TestContext) {
    measure(pool(&context).await, fifo("throughput-fifo"), Some(2_000)).await;
}
