use crate::ports::PersonalizedProductDetailsReadModel;
use common::error::boxed::BoxError;
use common::language::domain::Language;
use common::product_id::ProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductDetailsBatchReadRequest {
    pub user_id: UserId,
    pub language: Language,
    pub product_ids: Vec<ProductId>,
    pub search_filter_id: UserSearchFilterId,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsBatchReadError {
    #[error("product details batch query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product details batch read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductDetailsBatchReader: Send + Sync {
    async fn find_for_user(
        &self,
        request: &ProductDetailsBatchReadRequest,
    ) -> Result<HashMap<ProductId, PersonalizedProductDetailsReadModel>, ProductDetailsBatchReadError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::error::boxed::static_error;
    use std::error::Error;

    #[test]
    fn should_preserve_batch_query_failure_source() {
        let error = ProductDetailsBatchReadError::QueryFailed {
            source: static_error("database connection lost"),
        };

        assert_eq!(
            Some("database connection lost"),
            error.source().map(ToString::to_string).as_deref()
        );
    }

    #[test]
    fn should_preserve_invalid_batch_read_model_source() {
        let error = ProductDetailsBatchReadError::InvalidReadModel {
            source: static_error("persisted product details are invalid"),
        };

        assert_eq!(
            Some("persisted product details are invalid"),
            error.source().map(ToString::to_string).as_deref()
        );
    }
}
