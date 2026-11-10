use crate::{
    ports::ProductListingPriceFilterPlan,
    use_cases::queries::search_product_listings::ProductListingSearchItem,
};
use application::error::BoxError;
use localization::Language;
use product_listing_core::product_listing_id::ProductListingId;

/// How many neighbours a similarity lookup returns. A requested count outside the supported
/// range is clamped into it rather than rejected, like list sizes elsewhere.
///
/// `MAX` stays below the index's `knn.algo_param.ef_search` (see
/// `opensearch/mappings/product_listings.json`), so one approximate query can fill it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimilarProductListingsLimit(u64);

impl SimilarProductListingsLimit {
    pub const MIN: u64 = 1;
    pub const MAX: u64 = 50;
    pub const DEFAULT: u64 = 20;

    pub fn clamped(requested: u64) -> Self {
        Self(requested.clamp(Self::MIN, Self::MAX))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for SimilarProductListingsLimit {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSimilarProductListingsRequest {
    pub product_listing_id: ProductListingId,
    pub embedding: Vec<f32>,
    pub language: Language,
    pub price_filter_plan: ProductListingPriceFilterPlan,
    pub limit: SimilarProductListingsLimit,
}

impl ProductListingSimilarProductListingsRequest {
    pub fn new(
        product_listing_id: ProductListingId,
        embedding: Vec<f32>,
        language: Language,
        price_filter_plan: ProductListingPriceFilterPlan,
        limit: SimilarProductListingsLimit,
    ) -> Self {
        Self {
            product_listing_id,
            embedding,
            language,
            price_filter_plan,
            limit,
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
    ) -> Result<Vec<ProductListingSearchItem>, ProductListingSimilarProductListingsReadError>;
}

#[cfg(test)]
mod tests {
    use super::SimilarProductListingsLimit;

    #[test]
    fn should_default_to_twenty_neighbours() {
        assert_eq!(20, SimilarProductListingsLimit::default().get());
    }

    #[test]
    fn should_keep_requested_counts_inside_the_supported_range() {
        for requested in [1, 20, 50] {
            assert_eq!(
                requested,
                SimilarProductListingsLimit::clamped(requested).get()
            );
        }
    }

    #[test]
    fn should_clamp_requested_counts_outside_the_supported_range() {
        assert_eq!(1, SimilarProductListingsLimit::clamped(0).get());
        assert_eq!(50, SimilarProductListingsLimit::clamped(51).get());
        assert_eq!(50, SimilarProductListingsLimit::clamped(u64::MAX).get());
    }
}
