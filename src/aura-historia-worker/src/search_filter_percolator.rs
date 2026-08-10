use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    postgres::SqlxUnitOfWork,
    product_id::ProductId,
    transaction::{Transaction, UnitOfWork},
};
use product_postgres::SqlxProductSearchFilterMatchSourceReaderFactory;
use product_service::ports::{
    ProductSearchFilterMatchSourceReadError, ProductSearchFilterMatchSourceReader,
    ProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_service::{
    ports::{
        EnhancedSearchFilterEvaluation, EnhancedSearchFilterEvaluator,
        EnhancedSearchFilterEvaluatorError, SearchFilterView,
    },
    use_cases::{MatchProductEventCommand, MatchProductEventUseCase},
};
use std::sync::Arc;
use tracing::{error, info};

/// Temporary evaluator. This is deliberately not a matcher.
///
/// Until the canonical Gemini adapter exists, enhanced filters must fail closed rather than
/// becoming regular percolator matches. This makes the missing capability visible through retry
/// and DLQ logs without taking a dependency on the legacy evaluator.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedEnhancedSearchFilterEvaluator;

#[async_trait::async_trait]
impl EnhancedSearchFilterEvaluator for FailClosedEnhancedSearchFilterEvaluator {
    async fn evaluate(
        &self,
        _product: &product_service::ports::ProductSearchFilterMatchSource,
        _filter: &SearchFilterView,
    ) -> Result<EnhancedSearchFilterEvaluation, EnhancedSearchFilterEvaluatorError> {
        Err(unavailable_enhanced_evaluator_error())
    }
}

pub async fn consume_search_filter_percolator_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    handler: Arc<dyn MatchProductEventUseCase>,
    source_unit_of_work: SqlxUnitOfWork,
    source_reader_factory: SqlxProductSearchFilterMatchSourceReaderFactory,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let handler_for_retry = Arc::clone(&handler);
        let source_unit_of_work_for_retry = source_unit_of_work.clone();
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let handler = Arc::clone(&handler_for_retry);
            let source_unit_of_work = source_unit_of_work_for_retry.clone();
            async move {
                match_product_event(handler, source_unit_of_work, source_reader_factory, job).await
            }
        })
        .await;

        match result {
            Ok(()) => info!(
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                outcome = "applied",
                "search filter percolator job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter percolator job failed"
            ),
        }
    }
}

async fn match_product_event(
    handler: Arc<dyn MatchProductEventUseCase>,
    source_unit_of_work: SqlxUnitOfWork,
    source_reader_factory: SqlxProductSearchFilterMatchSourceReaderFactory,
    job: DomainJob,
) -> Result<(), SearchFilterPercolatorWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(SearchFilterPercolatorWorkerError::UnexpectedJobPayload);
    };
    let event_id = EventId::try_from(event.event_id.as_str()).map_err(|source| {
        SearchFilterPercolatorWorkerError::InvalidEventId {
            source: box_error(source),
        }
    })?;
    let product_id = ProductId::try_from(event.product_id.as_str()).map_err(|source| {
        SearchFilterPercolatorWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;

    let mut source_tx = source_unit_of_work.begin().await.map_err(|source| {
        SearchFilterPercolatorWorkerError::BeginSourceReadTransaction {
            source: box_error(source),
        }
    })?;
    let product = source_reader_factory
        .in_transaction(&mut source_tx)
        .find_source(event_id, product_id)
        .await
        .map_err(source_read_error)?
        .ok_or(SearchFilterPercolatorWorkerError::SourceNotFound {
            event_id,
            product_id,
        })?;
    let occurred_at = product.updated;
    source_tx.commit().await.map_err(|source| {
        SearchFilterPercolatorWorkerError::CommitSourceReadTransaction {
            source: box_error(source),
        }
    })?;

    handler
        .execute(MatchProductEventCommand {
            origin_event_id: event_id,
            occurred_at,
            product,
        })
        .await
        .map(|result| {
            info!(
                job_type = "search_filter_percolator",
                %event_id,
                %product_id,
                percolated_count = result.percolated_count,
                persisted_match_count = result.persisted_match_count,
                outcome = "applied",
                "search filter product matching completed"
            );
        })
        .map_err(|source| {
            error!(
                %event_id,
                %product_id,
                error = %source,
                "search filter product matching failed"
            );
            SearchFilterPercolatorWorkerError::Match {
                source: box_error(source),
            }
        })
}

fn source_read_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> SearchFilterPercolatorWorkerError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            SearchFilterPercolatorWorkerError::SourceStateInvalid { source }
        }
        error => SearchFilterPercolatorWorkerError::ReadSource {
            source: box_error(error),
        },
    }
}

fn unavailable_enhanced_evaluator_error() -> EnhancedSearchFilterEvaluatorError {
    EnhancedSearchFilterEvaluatorError::EvaluationFailed {
        source: box_error(std::io::Error::other(
            "canonical Gemini enhanced search-filter evaluator is not configured",
        )),
    }
}

#[derive(Debug, thiserror::Error)]
enum SearchFilterPercolatorWorkerError {
    #[error("search filter percolator queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("search filter percolator job has an invalid event id")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolator job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product source read transaction")]
    BeginSourceReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("product source read failed")]
    ReadSource {
        #[source]
        source: BoxError,
    },
    #[error("product source persisted state is invalid")]
    SourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product source was not found for event {event_id} and product {product_id}")]
    SourceNotFound {
        event_id: EventId,
        product_id: ProductId,
    },
    #[error("failed to commit product source read transaction")]
    CommitSourceReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("search filter product matching failed")]
    Match {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_describe_unavailable_enhanced_evaluator_as_a_failure() {
        assert!(matches!(
            unavailable_enhanced_evaluator_error(),
            EnhancedSearchFilterEvaluatorError::EvaluationFailed { .. }
        ));
    }
}
