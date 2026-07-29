#![allow(dead_code)]

use crate::use_cases::queries::get_product::{GetProductRequest, ProductDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsReadError {
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductDetailsReader: Send {
    async fn find_details(
        &mut self,
        request: &GetProductRequest,
    ) -> Result<Option<ProductDetailsView>, ProductDetailsReadError>;
}

pub trait ProductDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductDetailsReader + 'tx;
}
