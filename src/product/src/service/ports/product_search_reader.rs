#![allow(dead_code)]

use crate::service::use_cases::queries::search_products::{
    SearchProductsRequest, SearchProductsResult,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchReadError {
    #[error("temporary product search read failure")]
    TemporarilyUnavailable,
    #[error("invalid product search read model")]
    InvalidReadModel,
    #[error("internal product search read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ProductSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchProductsRequest,
    ) -> Result<SearchProductsResult, ProductSearchReadError>;
}
