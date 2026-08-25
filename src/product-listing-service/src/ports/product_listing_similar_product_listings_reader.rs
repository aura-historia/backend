use crate::{ports::ProductListingPriceFilterPlan, use_cases::ProductListingSummary};
use application::error::BoxError;
use localization::Language;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSimilarProductListingsRequest {
    pub product_id: ProductListingId,
    pub embedding: Vec<f32>,
    pub language: Language,
    pub price_filter_plan: ProductListingPriceFilterPlan,
}

impl ProductListingSimilarProductListingsRequest {
    pub fn new(
        product_id: ProductListingId,
        embedding: Vec<f32>,
        language: Language,
        price_filter_plan: ProductListingPriceFilterPlan,
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
pub enum ProductListingSimilarProductListingsReadError {
    #[error("similar products query failed")]
    SimilarProductListingsQueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingSimilarProductListingsReader: Send + Sync {
    async fn find_similar_product_listings(
        &self,
        request: &ProductListingSimilarProductListingsRequest,
    ) -> Result<Vec<ProductListingSummary>, ProductListingSimilarProductListingsReadError>;
}
