use crate::use_cases::ProductSummary;
use common::{
    currency::domain::Currency, error::boxed::BoxError, language::domain::Language,
    product_id::ProductId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSimilarProductsRequest {
    pub product_id: ProductId,
    pub embedding: Vec<f32>,
    pub language: Language,
    pub currency: Currency,
}

impl ProductSimilarProductsRequest {
    pub fn new(product_id: ProductId, embedding: Vec<f32>, language: Language) -> Self {
        Self {
            product_id,
            embedding,
            language,
            currency: Currency::Eur,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSimilarProductsReadError {
    #[error("similar products query failed")]
    SimilarProductsQueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductSimilarProductsReader: Send + Sync {
    async fn find_similar_products(
        &self,
        request: &ProductSimilarProductsRequest,
    ) -> Result<Vec<ProductSummary>, ProductSimilarProductsReadError>;
}
