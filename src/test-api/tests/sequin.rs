use std::time::Duration;

use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::{QueueConfig, WorkerRuntime};
use sqlx::Executor;
use test_api::*;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA, Sequin::worker_webhook()])]
async fn should_deliver_product_event_change_to_worker_queues() {
    let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(8)).unwrap();
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
        recv_or_dump_sequin_logs(&mut receivers, WorkerQueue::ProductOpenSearch, sequin).await;
    let percolator_job =
        recv_or_dump_sequin_logs(&mut receivers, WorkerQueue::SearchFilterPercolator, sequin).await;

    let _send_result = shutdown_tx.send(());
    server.await.unwrap().unwrap();

    assert!(product_job.is_some());
    assert!(percolator_job.is_some());
}

async fn recv_or_dump_sequin_logs(
    receivers: &mut aura_historia_worker::cdc::WorkerQueueReceivers,
    queue: WorkerQueue,
    sequin: &RunningSequin,
) -> Option<aura_historia_worker::cdc::DomainJob> {
    match receivers.recv_timeout(queue, Duration::from_secs(60)).await {
        Ok(job) => job,
        Err(error) => {
            panic!(
                "timed out waiting for Sequin job on {queue:?}: {error}\nstdout:\n{}\nstderr:\n{}",
                sequin.stdout_string().await,
                sequin.stderr_string().await
            )
        }
    }
}
