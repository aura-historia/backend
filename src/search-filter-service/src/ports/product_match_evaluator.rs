use crate::ports::SearchFilterView;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::error::boxed::BoxError;
use product_service::ports::ProductSearchFilterMatchSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductMatchEvaluation {
    Matched { reason: Option<EnhancedMatchReason> },
    NotMatched,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductMatchEvaluatorError {
    #[error("product match evaluation timed out")]
    Timeout {
        #[source]
        source: BoxError,
    },
    #[error("product match provider is temporarily unavailable")]
    Retryable {
        #[source]
        source: BoxError,
    },
    #[error("product match provider rejected the request")]
    Permanent {
        #[source]
        source: BoxError,
    },
    #[error("product match provider returned an invalid response")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

impl ProductMatchEvaluatorError {
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. } | Self::Retryable { .. } | Self::InvalidResponse { .. }
        )
    }
}

#[derive(Debug)]
pub struct ProductMatchEvaluationResult {
    pub search_filter_id: common::user_search_filter_id::UserSearchFilterId,
    pub result: Result<ProductMatchEvaluation, ProductMatchEvaluatorError>,
}

#[async_trait::async_trait]
pub trait ProductMatchEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError>;

    /// Evaluates one product's enhanced filters. Implementations may batch shared
    /// product preparation and must return a result for every submitted filter.
    async fn evaluate_batch(
        &self,
        product: &ProductSearchFilterMatchSource,
        filters: &[SearchFilterView],
    ) -> Vec<ProductMatchEvaluationResult> {
        let mut evaluations = Vec::with_capacity(filters.len());
        for filter in filters {
            evaluations.push(ProductMatchEvaluationResult {
                search_filter_id: filter.search_filter_id,
                result: self.evaluate(product, filter).await,
            });
        }
        evaluations
    }
}
