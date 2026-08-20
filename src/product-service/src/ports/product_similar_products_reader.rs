use crate::{ports::ProductPriceFilterPlan, use_cases::ProductSummary};
use common::error::boxed::BoxError;
use localization::Language;
use product_core::product_id::ProductId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSimilarProductsRequest {
    pub product_id: ProductId,
    pub embedding: Vec<f32>,
    pub language: Language,
    pub price_filter_plan: ProductPriceFilterPlan,
}

impl ProductSimilarProductsRequest {
    pub fn new(
        product_id: ProductId,
        embedding: Vec<f32>,
        language: Language,
        price_filter_plan: ProductPriceFilterPlan,
    ) -> Self {
        Self {
            product_id,
            embedding,
            language,
            price_filter_plan,
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
