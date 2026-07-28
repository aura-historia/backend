#![allow(dead_code)]

use crate::service::use_cases::queries::get_product::{GetProductRequest, ProductDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsReadError {
    #[error("temporary product details read failure")]
    TemporarilyUnavailable,
    #[error("invalid product details read model")]
    InvalidReadModel,
    #[error("internal product details read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ProductDetailsReader: Send + Sync {
    async fn find_details(
        &self,
        request: &GetProductRequest,
    ) -> Result<Option<ProductDetailsView>, ProductDetailsReadError>;
}
