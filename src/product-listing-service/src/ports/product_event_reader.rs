use crate::use_cases::queries::get_product_events::{ProductEvent, ProductEventLookup};

#[derive(Debug, thiserror::Error)]
pub enum ProductEventReadError {
    #[error("product event query failed")]
    ProductEventQueryFailed,
    #[error("product event read model is invalid")]
    ProductEventReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductEventReader: Send {
    async fn find_domain_events(
        &mut self,
        lookup: &ProductEventLookup,
    ) -> Result<Option<Vec<ProductEvent>>, ProductEventReadError>;
}

pub trait ProductEventReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductEventReader + 'tx;
}
