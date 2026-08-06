use crate::use_cases::queries::get_product::PersonalizedProductDetailsView;
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
    QueryFailed,
    #[error("product details batch read model is invalid")]
    InvalidReadModel,
}

#[async_trait::async_trait]
pub trait ProductDetailsBatchReader: Send + Sync {
    async fn find_for_user(
        &self,
        request: &ProductDetailsBatchReadRequest,
    ) -> Result<HashMap<ProductId, PersonalizedProductDetailsView>, ProductDetailsBatchReadError>;
}
