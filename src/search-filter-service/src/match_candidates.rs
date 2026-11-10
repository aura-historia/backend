//! Candidate retrieval and eligibility shared by every saved-search match decision.
//!
//! Scheduled matching and match previews retrieve, skip and classify candidates through these
//! functions, so a preview accepts exactly the candidates a scheduled run would accept for the
//! same intent and the same retrieved listings.

use application::pagination::Cursor;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceRef, ProductListingSearchReadError,
    ProductListingSearchReadRequest, ProductListingSearchReader,
};
use product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchItem;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;

/// Retrieved candidates that may still match: distinct and not withdrawn, in rank order.
pub(crate) struct RetrievedCandidates {
    pub(crate) items: Vec<ProductListingSearchItem>,
    pub(crate) scanned: usize,
    pub(crate) duplicates: usize,
    pub(crate) withdrawn: usize,
    pub(crate) scan_limit_reached: bool,
}

pub(crate) enum CandidateEligibility<'a> {
    Eligible(&'a ProductListingSearchFilterMatchSource),
    MissingSource,
    /// The search projection lags the listing's current event.
    Stale,
    Withdrawn,
}

#[derive(Debug, thiserror::Error)]
#[error("ProductListing source identity does not match its requested reference")]
pub(crate) struct CandidateSourceMismatch;

/// Only text and enhanced intent need the classifier; other intent matches on retrieval alone.
pub(crate) fn has_semantic_intent(search: &ProductListingSearch) -> bool {
    search.enhanced_search_description.is_some()
        || search
            .product_listing_query
            .iter()
            .any(|query| !query.trim().is_empty())
}

/// Enhanced intent retrieves with the filter's embedding; other intent retrieves lexically.
pub(crate) fn retrieves_with_embedding(search: &ProductListingSearch) -> bool {
    search.enhanced_search_description.is_some()
}

pub(crate) async fn retrieve_candidates<P>(
    reader: &P,
    compiled_search: CompiledProductListingSearch,
    embedding: Option<&[f32]>,
    scan_limit: NonZeroUsize,
) -> Result<RetrievedCandidates, ProductListingSearchReadError>
where
    P: ProductListingSearchReader,
{
    let request = ProductListingSearchReadRequest {
        compiled_search,
        sort: None,
        cursor: Some(Cursor {
            size: scan_limit.get() as u64,
            search_after: None,
        }),
    };
    let result = match embedding {
        Some(embedding) => reader.search_hybrid(&request, embedding).await,
        None => reader.search(&request).await,
    }?;
    let scanned = result.items.len().min(scan_limit.get());
    let mut retrieved = RetrievedCandidates {
        items: Vec::with_capacity(scanned),
        scanned,
        duplicates: 0,
        withdrawn: 0,
        scan_limit_reached: scanned == scan_limit.get(),
    };
    let mut seen = HashSet::new();
    for item in result.items.into_iter().take(scanned) {
        if !seen.insert(item.product_listing_id) {
            retrieved.duplicates += 1;
        } else if is_withdrawn_candidate(item.lifecycle) {
            retrieved.withdrawn += 1;
        } else {
            retrieved.items.push(item);
        }
    }
    Ok(retrieved)
}

pub(crate) fn candidate_eligibility<'a>(
    reference: &ProductListingSearchFilterMatchSourceRef,
    sources: &'a HashMap<
        ProductListingSearchFilterMatchSourceRef,
        ProductListingSearchFilterMatchSource,
    >,
) -> Result<CandidateEligibility<'a>, CandidateSourceMismatch> {
    match sources.get(reference) {
        None => Ok(CandidateEligibility::MissingSource),
        Some(source)
            if source.product_listing_id != reference.product_listing_id
                || source.event_id != reference.event_id =>
        {
            Err(CandidateSourceMismatch)
        }
        Some(source) if source.current_event_id != reference.event_id => {
            Ok(CandidateEligibility::Stale)
        }
        Some(source) if is_withdrawn_candidate(source.lifecycle) => {
            Ok(CandidateEligibility::Withdrawn)
        }
        Some(source) => Ok(CandidateEligibility::Eligible(source)),
    }
}

fn is_withdrawn_candidate(lifecycle: ListingLifecycle) -> bool {
    lifecycle == ListingLifecycle::Withdrawn
}
