#![allow(dead_code)]

use crate::use_cases::queries::search_products::{SearchProductsRequest, SearchProductsResult};

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchReadError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductSearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchProductsRequest,
    ) -> Result<SearchProductsResult, ProductSearchReadError>;
}

pub trait ProductSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductSearchReader + 'tx;
}
