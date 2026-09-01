use crate::use_cases::queries::get_product_listing_history::{
    ProductListingHistoryEntry, ProductListingHistoryLookup,
};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingHistoryReadError {
    #[error("product listing history query failed")]
    ProductListingHistoryQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product listing history read model is invalid")]
    ProductListingHistoryReadModelInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingHistoryReader: Send {
    async fn find_history(
        &mut self,
        lookup: &ProductListingHistoryLookup,
    ) -> Result<Option<Vec<ProductListingHistoryEntry>>, ProductListingHistoryReadError>;
}

pub trait ProductListingHistoryReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingHistoryReader + 'tx;
}
