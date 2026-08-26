use crate::use_cases::queries::get_product_listing_events::{
    ProductListingEvent, ProductListingEventLookup,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEventReadError {
    #[error("product event query failed")]
    ProductListingEventQueryFailed,
    #[error("product event read model is invalid")]
    ProductListingEventReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductListingEventReader: Send {
    async fn find_domain_events(
        &mut self,
        lookup: &ProductListingEventLookup,
    ) -> Result<Option<Vec<ProductListingEvent>>, ProductListingEventReadError>;
}

pub trait ProductListingEventReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEventReader + 'tx;
}
