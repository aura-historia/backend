use crate::user_state::ProductListingUserState;
use application::error::BoxError;
use product_listing_core::product_listing_id::ProductListingId;
use std::collections::HashMap;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingUserStateLookup {
    pub user_id: UserId,
    pub product_listing_ids: Vec<ProductListingId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingUserStateReadError {
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
pub trait ProductListingUserStateReader: Send + Sync {
    async fn find_for_user(
        &self,
        lookup: &ProductListingUserStateLookup,
    ) -> Result<HashMap<ProductListingId, ProductListingUserState>, ProductListingUserStateReadError>;
}
