#![allow(dead_code)]

use crate::use_cases::queries::search_products::{ProductSearchReadResult, SearchProductsRequest};

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchReadError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchProductsRequest,
    ) -> Result<ProductSearchReadResult, ProductSearchReadError>;

    async fn search_hybrid(
        &self,
        request: &SearchProductsRequest,
        embedding: &[f32],
    ) -> Result<ProductSearchReadResult, ProductSearchReadError>;
}
