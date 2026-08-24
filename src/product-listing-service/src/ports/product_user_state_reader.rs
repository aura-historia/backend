use crate::user_state::ProductUserState;
use application::error::BoxError;
use product_listing_core::product_id::ProductId;
use std::collections::HashMap;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductUserStateLookup {
    pub user_id: UserId,
    pub product_ids: Vec<ProductId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductUserStateReadError {
    #[error("product user state query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductUserStateReader: Send + Sync {
    async fn find_for_user(
        &self,
        lookup: &ProductUserStateLookup,
    ) -> Result<HashMap<ProductId, ProductUserState>, ProductUserStateReadError>;
}
