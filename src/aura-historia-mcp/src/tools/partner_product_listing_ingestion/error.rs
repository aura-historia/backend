use crate::OperationError;
use partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::PutListingSourceIngestionConfigurationError;
use product_listing_service::use_cases::ProductListingIngestionSubmissionError;

pub(super) fn submission(error: ProductListingIngestionSubmissionError) -> OperationError {
    match error {
        ProductListingIngestionSubmissionError::AuthenticationRequired
        | ProductListingIngestionSubmissionError::Forbidden => OperationError::Forbidden,
        ProductListingIngestionSubmissionError::PublisherNotStarted { source } => {
            OperationError::Unavailable(Box::new(source))
        }
        error @ ProductListingIngestionSubmissionError::InconsistentInputIndices => {
            OperationError::Internal(Box::new(error))
        }
    }
}
pub(super) fn configuration(error: PutListingSourceIngestionConfigurationError) -> OperationError {
    use PutListingSourceIngestionConfigurationError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        NotFound => OperationError::NotFound,
        InvalidConfiguration => OperationError::InvalidInput,
        ConcurrencyConflict | ShopifyDomainConflict { .. } => OperationError::Conflict(None),
        TemporarilyUnavailable { source }
        | BeginTransactionFailed(source)
        | CommitTransactionFailed(source) => OperationError::Unavailable(source),
        InvalidPersistedState { source } | Internal { source } => OperationError::Internal(source),
    }
}
