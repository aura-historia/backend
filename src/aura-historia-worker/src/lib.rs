//! The worker process: the outbox dispatcher, one job queue consumer per scope and the host
//! health metrics. The
//! [README](../README.md) owns its configuration and operation.

mod composition;
pub mod config;
pub mod dispatcher;
pub mod health_metrics;
pub mod scope;
pub mod scopes;
pub mod translation_batches;

pub use composition::{CompositionError, compose_handler};

use aura_historia_jobs::WorkerScope;

use config::WorkerConfig;
use delivery_outbox::OutboxDispatcher;
use platform_postgres_queue::JobQueue;
use platform_worker::{Consumer, JobHandler, ScopeWakeups};
use scope::ConsumedScope;
use sqlx::PgPool;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Composes every configured scope's handler. Any failure stops startup.
pub async fn compose_handlers(
    config: &WorkerConfig,
    pool: &PgPool,
) -> Result<Vec<(ConsumedScope, Arc<dyn JobHandler>)>, CompositionError> {
    let mut handlers = Vec::with_capacity(config.scopes.len());
    for scope in &config.scopes {
        handlers.push((
            scope.scope.clone(),
            compose_handler(&scope.scope, pool).await?,
        ));
    }
    Ok(handlers)
}

/// A long-running task beside the consumers, such as the title translation rounds. It returns
/// once its shutdown token is cancelled.
pub type BackgroundTask =
    Box<dyn FnOnce(CancellationToken) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Composes the background tasks the configured scopes need. Any failure stops startup.
pub async fn compose_background_tasks(
    config: &WorkerConfig,
    pool: &PgPool,
) -> Result<Vec<BackgroundTask>, CompositionError> {
    let mut tasks = Vec::new();
    let translation = ConsumedScope::Worker(WorkerScope::ProductListingTranslation);
    if config.scopes.iter().any(|scope| scope.scope == translation) {
        let rounds = translation_batches::compose(pool).await?;
        tasks.push(translation_rounds(
            rounds,
            translation_batches::ROUND_INTERVAL,
        ));
    }
    Ok(tasks)
}

/// The title translation rounds as a background task.
pub fn translation_rounds(
    use_case: Arc<
        dyn product_listing_service::use_cases::TranslateProductListingTitleBatchesUseCase,
    >,
    interval: std::time::Duration,
) -> BackgroundTask {
    Box::new(move |shutdown| Box::pin(translation_batches::run(use_case, interval, shutdown)))
}

/// A consumer, the dispatcher or the wake-up listener stopped before shutdown was requested.
#[derive(Debug, thiserror::Error)]
#[error("a worker task stopped before shutdown; the worker stopped every task")]
pub struct WorkerTaskStopped;

/// Runs the dispatcher and the consumers until `shutdown`, then waits for each to drain. If any
/// task stops on its own, every other task is shut down as well and the error is returned, so the
/// process exits and is restarted instead of running with a scope that no longer consumes.
pub async fn run(
    pool: PgPool,
    config: WorkerConfig,
    handlers: Vec<(ConsumedScope, Arc<dyn JobHandler>)>,
    background: Vec<BackgroundTask>,
    shutdown: CancellationToken,
) -> Result<(), WorkerTaskStopped> {
    let requested = shutdown;
    let shutdown = requested.child_token();
    let mut tasks = JoinSet::new();
    let mut wakeups = ScopeWakeups::default();
    for (scope, handler) in handlers {
        let Some(scope_config) = config.scopes.iter().find(|c| c.scope == scope) else {
            continue;
        };
        let job_scope = scope.job_scope();
        let wakeup = wakeups.register(job_scope.name());
        let consumer = Consumer::new(
            JobQueue::new(pool.clone(), job_scope),
            handler,
            scope_config.consumer,
        );
        tracing::info!(
            event = "worker.consumer.started",
            job_scope = consumer.scope().name().as_str(),
            concurrency = scope_config.consumer.concurrency().get(),
            job_timeout_ms =
                u64::try_from(scope_config.consumer.job_timeout().as_millis()).unwrap_or(u64::MAX),
        );
        tasks.spawn(consumer.run(wakeup, shutdown.clone()));
    }
    tasks.spawn(wakeups.listen(pool.clone(), shutdown.clone()));
    for task in background {
        tasks.spawn(task(shutdown.clone()));
    }
    if let Some(metrics) = config.health_metrics.clone() {
        tracing::info!(event = "worker.health_metrics.started");
        let client = health_metrics::client().await;
        tasks.spawn(health_metrics::run(
            pool.clone(),
            metrics,
            client,
            shutdown.clone(),
        ));
    }
    if let Some(batch_size) = config.dispatch_batch_size {
        tracing::info!(
            event = "worker.dispatcher.started",
            batch_size = batch_size.get()
        );
        tasks.spawn(dispatcher::run(
            pool.clone(),
            OutboxDispatcher::new(pool, batch_size),
            usize::from(batch_size.get()),
            config.poll_interval,
            shutdown.clone(),
        ));
    }
    let mut stopped = false;
    while let Some(result) = tasks.join_next().await {
        if !requested.is_cancelled() && !stopped {
            stopped = true;
            tracing::error!(
                event = "worker.task.stopped",
                panicked = result.is_err(),
                "A worker task stopped before shutdown; stopping the worker"
            );
            shutdown.cancel();
        }
    }
    if stopped {
        Err(WorkerTaskStopped)
    } else {
        Ok(())
    }
}
