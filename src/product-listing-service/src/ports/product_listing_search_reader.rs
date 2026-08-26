#![allow(dead_code)]

use crate::{
    ports::ProductListingPriceFilterPlan,
    use_cases::queries::search_product_listings::ProductListingSearchReadResult,
};
use application::pagination::Cursor;
use domain_primitives::sort::Sort;
use product_listing_core::{
    product_listing_search::ProductListingSearch,
    sort_product_listing_field::SortProductListingField,
};
use serde_json::Value;

/// A ProductListing search compiled against one persisted FX snapshot.
///
/// The raw `ProductListingSearch` is retained for non-price filters. OpenSearch adapters must render
/// price clauses only from `price_filter_plan`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledProductListingSearch {
    pub search: ProductListingSearch,
    pub price_filter_plan: ProductListingPriceFilterPlan,
}

/// One adapter-facing ProductListing search request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSearchReadRequest {
    pub compiled_search: CompiledProductListingSearch,
    pub sort: Option<Sort<SortProductListingField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchReadError {
    #[error("product search query failed")]
    ProductListingSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductListingSearchReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductListingSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &ProductListingSearchReadRequest,
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError>;

    async fn search_hybrid(
        &self,
        request: &ProductListingSearchReadRequest,
        embedding: &[f32],
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError>;
}
