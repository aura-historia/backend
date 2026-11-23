//! The product embedding sweep before the daily periodic match: embeds the work still unsettled
//! synchronously, so the match sees it ([embeddings](../../../docs/product-listing.md#embeddings)).

use crate::OpsError;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use embedding::{VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator};
use platform_postgres::SqlxUnitOfWork;
use platform_process::{config, google_application_default_credentials};
use product_listing_postgres::{
    SqlxProductListingEmbeddingQueueFactory, SqlxProductListingEmbeddingQueueReader,
    SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    ProductEmbeddingSweepReport, ProductEmbeddingWorkError, SweepProductListingEmbeddingsHandler,
    SweepProductListingEmbeddingsUseCase,
};
use serde_json::{Value, json};

const COMMAND: &str = "embedding-sweep";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";

pub(crate) async fn run() -> Result<Value, OpsError> {
    let setting = |name| config::required(name).map_err(|error| OpsError::Config(error.into()));
    let embedding_config = VertexAiEmbeddingConfig::new(
        setting(VERTEX_AI_PROJECT_ID_ENV)?,
        setting(VERTEX_AI_LOCATION_ENV)?,
    );
    let credentials =
        google_application_default_credentials().map_err(|error| OpsError::Config(error.into()))?;
    let generator = VertexAiEmbeddingGenerator::new(embedding_config, credentials)
        .map_err(|error| OpsError::failed(COMMAND, error))?;
    let pool = crate::pool().await?;
    let sweep = SweepProductListingEmbeddingsHandler::new(
        SqlxProductListingEmbeddingQueueReader::new(pool.clone()),
        generator,
        SqlxUnitOfWork::new(pool),
        SqlxProductListingEmbeddingQueueFactory::new(),
        SqlxProductListingEmbeddingWriterFactory::new(),
    );
    sweep_pending_embeddings(&sweep).await
}

/// Runs one sweep. It fails when Vertex AI answered none of the work it swept, or when a result
/// could not be settled: that work still misses the match, so the job's alarm must see it.
pub async fn sweep_pending_embeddings(
    sweep: &dyn SweepProductListingEmbeddingsUseCase,
) -> Result<Value, OpsError> {
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(COMMAND),
        correlation_id: CorrelationId::new(COMMAND),
    };
    let report = sweep
        .execute(&context)
        .await
        .map_err(|error| OpsError::failed(COMMAND, SweepFailed(error.kind(), error)))?;
    let settlements = report.settlements;
    tracing::info!(
        event = "product.embedding.sweep.completed",
        swept = report.swept,
        applied = settlements.applied,
        duplicate = settlements.duplicate,
        stale = settlements.stale,
        requeued = settlements.requeued,
        parked = settlements.parked,
        write_failures = settlements.write_failures,
        unavailable = report.unavailable,
        settlement_failures = report.settlement_failures,
        out_of_time = report.out_of_time,
    );
    if report.out_of_time > 0 {
        tracing::warn!(
            event = "product.embedding.sweep.out_of_time",
            out_of_time = report.out_of_time,
            "The sweep stopped before the match with work left unembedded"
        );
    }
    if provider_unavailable_throughout(&report) {
        return Err(OpsError::failed(COMMAND, ProviderUnavailable));
    }
    if report.settlement_failures > 0 {
        return Err(OpsError::failed(COMMAND, SettlementFailed));
    }
    Ok(json!({
        "outcome": "success",
        "swept": report.swept,
        "applied": settlements.applied,
        "duplicate": settlements.duplicate,
        "stale": settlements.stale,
        "requeued": settlements.requeued,
        "parked": settlements.parked,
        "writeFailures": settlements.write_failures,
        "unavailable": report.unavailable,
        "outOfTime": report.out_of_time,
    }))
}

fn provider_unavailable_throughout(report: &ProductEmbeddingSweepReport) -> bool {
    report.swept > 0 && report.unavailable == report.swept
}

/// Names only the failed step; the cause can carry provider text.
#[derive(Debug, thiserror::Error)]
#[error("the embedding sweep failed at {0}")]
struct SweepFailed(&'static str, #[source] ProductEmbeddingWorkError);

#[derive(Debug, thiserror::Error)]
#[error("Vertex AI was unavailable for every swept embedding")]
struct ProviderUnavailable;

#[derive(Debug, thiserror::Error)]
#[error("some swept embeddings could not be settled in PostgreSQL")]
struct SettlementFailed;

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_service::use_cases::ProductEmbeddingSettlements;

    fn report(swept: usize, unavailable: usize, applied: usize) -> ProductEmbeddingSweepReport {
        ProductEmbeddingSweepReport {
            swept,
            unavailable,
            settlements: ProductEmbeddingSettlements {
                applied,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn should_fail_only_when_the_provider_answered_no_swept_embedding() {
        assert!(provider_unavailable_throughout(&report(3, 3, 0)));
        assert!(!provider_unavailable_throughout(&report(3, 2, 1)));
        assert!(!provider_unavailable_throughout(&report(0, 0, 0)));
    }
}
