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
        ConcurrencyConflict => OperationError::Conflict(None),
        ShopifyDomainConflict { source } => OperationError::Conflict(Some(source)),
        TemporarilyUnavailable { source }
        | BeginTransactionFailed(source)
        | CommitTransactionFailed(source) => OperationError::Unavailable(source),
        InvalidPersistedState { source } | Internal { source } => OperationError::Internal(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn should_preserve_shopify_conflict_cause_without_exposing_it_in_tool_results() {
        let error = configuration(
            PutListingSourceIngestionConfigurationError::ShopifyDomainConflict {
                source: application::error::static_error("private provider token secret"),
            },
        );
        assert!(matches!(error, OperationError::Conflict(Some(_))));
        assert_eq!(
            error.source().unwrap().to_string(),
            "private provider token secret"
        );
        let result = serde_json::to_value(error.into_protocol().unwrap()).unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "CONFLICT");
        assert!(!result.to_string().contains("private"));
        assert!(!result.to_string().contains("secret"));
    }
}
