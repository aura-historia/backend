use std::time::Duration;

use aura_historia_worker::cdc::{CdcFanout, DomainJob, WorkerQueue, WorkerQueueRegistry};
use aura_historia_worker::{QueueConfig, WorkerRuntime, in_memory_queue};
use sqlx::Executor;
use test_api::*;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA, Sequin::worker_webhook()])]
async fn should_deliver_product_event_change_to_worker_queues() {
    let (product_sender, mut product_receiver) =
        in_memory_queue::<DomainJob>(QueueConfig::new(8)).unwrap();
    let (percolator_sender, mut percolator_receiver) =
        in_memory_queue::<DomainJob>(QueueConfig::new(8)).unwrap();
    let runtime = WorkerRuntime::new(CdcFanout::new(
        WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::ProductOpenSearch, product_sender)
            .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender),
    ));
    let listener = TcpListener::bind(get_sequin_worker_webhook_bind_addr())
        .await
        .unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(aura_historia_worker::serve_with_runtime(
        listener,
        runtime,
        async move {
            let _ = shutdown_rx.await;
        },
    ));
    let sequin = get_or_start_worker_webhook_sequin().await;

    let pool = get_postgres_client().await;
    let fixture = include_str!("fixtures/business_schema_relations.sql");
    pool.execute(sqlx::raw_sql(fixture)).await.unwrap();

    let product_job =
        match tokio::time::timeout(Duration::from_secs(60), product_receiver.recv()).await {
            Ok(job) => job,
            Err(error) => {
                panic!(
                    "timed out waiting for Sequin product job: {error}\nstdout:\n{}\nstderr:\n{}",
                    sequin.stdout_string().await,
                    sequin.stderr_string().await
                )
            }
        };
    let percolator_job = match tokio::time::timeout(
        Duration::from_secs(60),
        percolator_receiver.recv(),
    )
    .await
    {
        Ok(job) => job,
        Err(error) => {
            panic!(
                "timed out waiting for Sequin percolator job: {error}\nstdout:\n{}\nstderr:\n{}",
                sequin.stdout_string().await,
                sequin.stderr_string().await
            )
        }
    };

    let _send_result = shutdown_tx.send(());
    server.await.unwrap().unwrap();

    assert!(product_job.is_some());
    assert!(percolator_job.is_some());
}
