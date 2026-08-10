use crate::ports::SearchFilterView;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::error::boxed::BoxError;
use product_service::ports::ProductSearchFilterMatchSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnhancedSearchFilterEvaluation {
    Matched { reason: Option<EnhancedMatchReason> },
    NotMatched,
}

#[derive(Debug, thiserror::Error)]
pub enum EnhancedSearchFilterEvaluatorError {
    #[error("enhanced search filter evaluation failed")]
    EvaluationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait EnhancedSearchFilterEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<EnhancedSearchFilterEvaluation, EnhancedSearchFilterEvaluatorError>;
}
