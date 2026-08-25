use application::error::BoxError;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use std::collections::HashSet;

#[derive(Debug, thiserror::Error)]
pub enum ExistingSearchFilterMatchReadError {
    #[error("existing search-filter match read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ExistingSearchFilterMatchReader: Send + Sync {
    async fn find_existing_product_ids(
        &self,
        search_filter_id: UserSearchFilterId,
        product_ids: &[ProductListingId],
    ) -> Result<HashSet<ProductListingId>, ExistingSearchFilterMatchReadError>;
}
