#![allow(dead_code)]

use crate::service::use_cases::queries::get_product::{GetProductRequest, ProductDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsReadError {
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductDetailsReader: Send + Sync {
    async fn find_details(
        &self,
        request: &GetProductRequest,
    ) -> Result<Option<ProductDetailsView>, ProductDetailsReadError>;
}
