#![allow(dead_code)]

use crate::{
    ports::{NativePriceRange, ProductListingPriceFilterPlan},
    use_cases::queries::search_product_listings::ProductListingSearchReadResult,
};
use application::error::BoxError;
use application::pagination::Cursor;
use domain_primitives::sort::Sort;
use product_listing_core::{
    product_listing_search::ProductListingSearch,
    sort_product_listing_field::SortProductListingField,
};
use serde_json::Value;

/// A ProductListing search compiled against one persisted FX snapshot.
///
/// The raw `ProductListingSearch` retains non-price and source-currency shipping predicates.
/// Adapters render asking-price clauses from `price_filter_plan` and FX shipping clauses from
/// the precompiled shipping ranges, preserving service-owned conversion errors.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledProductListingSearch {
    pub search: ProductListingSearch,
    pub price_filter_plan: ProductListingPriceFilterPlan,
    shipping_native_ranges: Option<Vec<NativePriceRange>>,
}

impl CompiledProductListingSearch {
    pub fn new(
        search: ProductListingSearch,
        price_filter_plan: ProductListingPriceFilterPlan,
    ) -> Result<Self, fxrate_core::FxRateSnapshotError> {
        let shipping_native_ranges = search
            .shipping
            .as_ref()
            .map(|query| price_filter_plan.shipping_native_ranges(query))
            .transpose()?
            .flatten();
        Ok(Self {
            search,
            price_filter_plan,
            shipping_native_ranges,
        })
    }

    pub fn shipping_native_ranges(&self) -> Option<&[NativePriceRange]> {
        self.shipping_native_ranges.as_deref()
    }
}

/// One adapter-facing ProductListing search request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSearchReadRequest {
    pub compiled_search: CompiledProductListingSearch,
    pub sort: Option<Sort<SortProductListingField>>,
    pub cursor: Option<Cursor<Value>>,
}

/// Search strategy needed to resume an adapter-owned continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingSearchMode {
    Lexical,
    Hybrid,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchReadError {
    #[error("invalid product search continuation cursor")]
    InvalidCursor,
    #[error("product search query failed")]
    ProductListingSearchQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product search read model is invalid")]
    ProductListingSearchReadModelInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingSearchReader: Send + Sync {
    /// Validates an opaque continuation locally and identifies its search mode.
    ///
    /// This method must be pure and perform no dependency I/O, allowing callers to reject
    /// invalid continuations before reading other dependencies. Cursor encoding and
    /// sort-value compatibility remain adapter-owned; callers must not inspect the payload.
    fn continuation_mode(
        &self,
        search_after: &Value,
        sort_field: SortProductListingField,
    ) -> Result<ProductListingSearchMode, ProductListingSearchReadError>;

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
