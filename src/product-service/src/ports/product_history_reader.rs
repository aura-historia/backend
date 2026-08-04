use crate::use_cases::queries::get_product_history::ProductHistoryEvent;
use common::product_id::ProductKey;

#[derive(Debug, thiserror::Error)]
pub enum ProductHistoryReadError {
    #[error("product history query failed")]
    ProductHistoryQueryFailed,
    #[error("product history read model is invalid")]
    ProductHistoryReadModelInvalid,
    #[error("product history event schema is unsupported")]
    UnsupportedProductHistoryEventSchema,
}

#[async_trait::async_trait]
pub trait ProductHistoryReader: Send {
    async fn find_history(
        &mut self,
        product_key: &ProductKey,
    ) -> Result<Option<Vec<ProductHistoryEvent>>, ProductHistoryReadError>;
}

pub trait ProductHistoryReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductHistoryReader + 'tx;
}
