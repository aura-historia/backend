//! Batched product embedding: a round every [`ROUND_INTERVAL`] collects ended Vertex AI batch
//! prediction jobs and submits the pending work as a new job. It runs in every worker that
//! consumes `product-embedding`, whose jobs only record the work.

use crate::composition::CompositionError;
use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use embedding::{
    EmbeddingError, ProductEmbeddingBatches, VertexAiBatchConfig, VertexAiEmbeddingConfig,
    VertexAiProductEmbeddingBatches,
};
use platform_postgres::SqlxUnitOfWork;
use platform_process::{
    Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE, config::required,
    google_application_default_credentials,
};
use product_listing_postgres::{
    SqlxProductListingEmbeddingQueueFactory, SqlxProductListingEmbeddingQueueReader,
    SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    EmbedProductListingBatchesError, EmbedProductListingBatchesHandler,
    EmbedProductListingBatchesUseCase, ProductEmbeddingRoundReport,
};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const SCOPE: &str = "product-embedding";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
const VERTEX_AI_EMBEDDING_BATCH_BUCKET_ENV: &str = "VERTEX_AI_EMBEDDING_BATCH_BUCKET";
/// How often a round runs. Pending work accumulates in between; nothing needs its embedding
/// within minutes. A listing still unembedded at the daily match is found by the next one.
pub const ROUND_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Builds the PostgreSQL-backed round with Vertex AI batch prediction.
pub async fn compose(
    pool: &PgPool,
) -> Result<Arc<dyn EmbedProductListingBatchesUseCase>, CompositionError> {
    let setting = |name| required(name).map_err(|e| CompositionError::new(SCOPE, e));
    let config = VertexAiBatchConfig::new(
        VertexAiEmbeddingConfig::new(
            setting(VERTEX_AI_PROJECT_ID_ENV)?,
            setting(VERTEX_AI_LOCATION_ENV)?,
        ),
        setting(VERTEX_AI_EMBEDDING_BATCH_BUCKET_ENV)?,
    );
    let credentials =
        google_application_default_credentials().map_err(|e| CompositionError::new(SCOPE, e))?;
    let batches = VertexAiProductEmbeddingBatches::new(config, credentials)
        .map_err(|e| CompositionError::new(SCOPE, e))?;
    Ok(compose_round_use_case(pool.clone(), batches))
}

pub fn compose_round_use_case<B>(
    pool: PgPool,
    batches: B,
) -> Arc<dyn EmbedProductListingBatchesUseCase>
where
    B: ProductEmbeddingBatches + 'static,
{
    Arc::new(EmbedProductListingBatchesHandler::new(
        SqlxProductListingEmbeddingQueueReader::new(pool.clone()),
        batches,
        SqlxUnitOfWork::new(pool),
        SqlxProductListingEmbeddingQueueFactory::new(),
        SqlxProductListingEmbeddingWriterFactory::new(),
    ))
}

/// Runs a round at once and then every `interval` until `shutdown`. A failed round leaves its
/// work durable in the queue; the next round continues from there.
pub async fn run(
    use_case: Arc<dyn EmbedProductListingBatchesUseCase>,
    interval: Duration,
    shutdown: CancellationToken,
) {
    let mut round = 0_u64;
    while !shutdown.is_cancelled() {
        round += 1;
        let context = OperationContext {
            principal: Principal::System,
            request_id: RequestId::new(format!("product-embedding-round:{round}")),
            correlation_id: CorrelationId::new(format!("product-embedding-round:{round}")),
        };
        let outcome = tokio::select! {
            () = shutdown.cancelled() => return,
            outcome = use_case.execute(&context) => outcome,
        };
        match outcome {
            Ok(report) => log_round(&report),
            Err(error) => log_failed_round(&error),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
    }
}

fn log_round(report: &ProductEmbeddingRoundReport) {
    let settlements = &report.settlements;
    tracing::info!(
        event = "product.embedding.round.completed",
        abandoned_batches = report.abandoned_batches,
        batches_in_progress = report.batches_in_progress,
        batches_unavailable = report.batches_unavailable,
        batches_collected = report.batches_collected,
        batches_failed = report.batches_failed,
        applied = settlements.applied,
        duplicate = settlements.duplicate,
        stale = settlements.stale,
        requeued = settlements.requeued,
        parked = settlements.parked,
        write_failures = settlements.write_failures,
        superseded_discarded = report.superseded_discarded,
        submitted_embeddings = report.submitted_embeddings,
    );
}

/// Records the failed stage and safe cause facts; the error chain itself is never logged.
fn log_failed_round(error: &EmbedProductListingBatchesError) {
    let Some(failure) = round_failure(error) else {
        tracing::error!(
            event = "product.embedding.round.failed",
            stage = "authorization",
            "Embedding round was not authorized"
        );
        return;
    };
    tracing::warn!(
        event = "product.embedding.round.failed",
        stage = failure.stage(),
        dependency = failure.dependency(),
        error_category = failure.category(),
        http_status = failure.http_status(),
        sqlstate = failure.sqlstate(),
        "Embedding round failed; the next round continues its work"
    );
}

fn round_failure(error: &EmbedProductListingBatchesError) -> Option<FailureDiagnostic> {
    use EmbedProductListingBatchesError as E;
    match error {
        E::SystemPrincipalRequired => None,
        E::SubmissionFailed { .. } => {
            let failure = find_source::<EmbeddingError>(error);
            Some(FailureDiagnostic::new(
                "batch_submit",
                Dependency::VertexAi,
                failure.map_or(UNCLASSIFIED_FAILURE, EmbeddingError::diagnostic_category),
                failure.and_then(EmbeddingError::http_status),
            ))
        }
        E::QueueFailed { .. }
        | E::BeginTransactionFailed { .. }
        | E::EmbeddingWriteFailed { .. }
        | E::CommitTransactionFailed { .. } => {
            Some(FailureDiagnostic::postgres(error.kind(), error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;

    struct FailingRounds {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl EmbedProductListingBatchesUseCase for FailingRounds {
        async fn execute(
            &self,
            _: &OperationContext,
        ) -> Result<ProductEmbeddingRoundReport, EmbedProductListingBatchesError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(EmbedProductListingBatchesError::SubmissionFailed {
                source: Box::new(EmbeddingError::AuthenticationFailed {
                    source: Box::new(std::io::Error::other("SENTINEL token detail")),
                }),
            })
        }
    }

    #[tokio::test]
    async fn should_keep_running_rounds_after_a_failure_and_log_only_safe_facts() {
        let rounds = Arc::new(FailingRounds {
            calls: AtomicUsize::new(0),
        });
        let shutdown = CancellationToken::new();
        let (logs, guard) = CapturedLogs::start();
        let task = tokio::spawn(run(
            rounds.clone(),
            Duration::from_millis(10),
            shutdown.clone(),
        ));

        while rounds.calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the round loop stops on shutdown")
            .unwrap();
        drop(guard);

        let failed = logs.events_where("event", "product.embedding.round.failed");
        assert!(failed.len() >= 2);
        assert_eq!(Some("batch_submit"), failed[0]["stage"].as_str());
        assert_eq!(Some("vertex_ai"), failed[0]["dependency"].as_str());
        assert_eq!(Some("authentication"), failed[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
