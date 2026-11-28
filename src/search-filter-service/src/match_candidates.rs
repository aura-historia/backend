//! Candidate retrieval and eligibility shared by every saved-search match decision.
//!
//! Scheduled matching and match previews retrieve, skip and classify candidates through these
//! functions, so a preview accepts exactly the candidates a scheduled run would accept for the
//! same intent and the same retrieved listings.

use crate::product_match_evaluator::candidate_title;
use application::pagination::Cursor;
use product_listing_core::listing_discoverability::is_discoverable;
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
    /// Withdrawn, or no longer discoverable (docs/product-listing.md): a frozen selection or a
    /// lagging index can still name such a listing, and it must never match.
    Undiscoverable,
}

#[derive(Debug, thiserror::Error)]
#[error("ProductListing source identity does not match its requested reference")]
pub(crate) struct CandidateSourceMismatch;

/// Text and enhanced intent retrieve with the embedding of that text, hybrid like the public
/// product search, and need the classifier. Other intent matches on lexical retrieval alone.
pub(crate) fn has_semantic_intent(search: &ProductListingSearch) -> bool {
    search.enhanced_search_description.is_some()
        || search
            .product_listing_query
            .iter()
            .any(|query| !query.trim().is_empty())
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
        Some(source) if !is_discoverable(source.lifecycle, source.availability) => {
            Ok(CandidateEligibility::Undiscoverable)
        }
        Some(source) => Ok(CandidateEligibility::Eligible(source)),
    }
}

/// How a listing's match evidence changed between a decision's event and a later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchEvidenceChange {
    /// Nothing retrieval filters on or the classifier reads changed, as when an embedding lands.
    Unchanged,
    /// Only the English title the classifier reads changed, as when a title translation lands.
    ClassifierTitle,
    /// A fact retrieval filters on, or other classifier evidence, changed.
    Decision,
}

/// Compares the evidence of a decision for `search` with the listing's later state. The
/// destructuring is exhaustive so that a new source field must be classified here.
pub(crate) fn match_evidence_change(
    search: &ProductListingSearch,
    decided: &ProductListingSearchFilterMatchSource,
    current: &ProductListingSearchFilterMatchSource,
) -> MatchEvidenceChange {
    let ProductListingSearchFilterMatchSource {
        // Which event the evidence belongs to, not the evidence itself.
        event_id: _,
        event_kind: _,
        origin_event_time: _,
        current_event_id: _,
        projection_version: _,
        // The embedding only ranks retrieval.
        embedding: _,
        // Translations serve search; the classifier reads only the English title, below.
        titles: _,
        // Presentation, read again when the match is delivered.
        product_listing_title_slug_id: _,
        source_listing_id: _,
        url: _,
        view_link: _,
        source,
        product_listing_id,
        geography,
        shipping,
        pricing,
        sale_observation,
        availability,
        lifecycle,
        auction,
        created,
        updated,
        product_title,
        product_description,
        descriptions,
        image,
        images,
    } = current;
    // Shipping and geography enrichment matter only to filters that constrain them; freshness
    // only to an authored upper bound, since the matching window itself ends before the run.
    let same_shipping = search.shipping.is_none() || *shipping == decided.shipping;
    let same_geography = (search.geography.is_none() && search.shipping.is_none())
        || *geography == decided.geography;
    let within_authored_updated = search
        .updated_query
        .as_ref()
        .and_then(|range| range.max)
        .is_none_or(|max| *updated <= max);
    let same_decision_facts = *product_listing_id == decided.product_listing_id
        && source.listing_source_id == decided.source.listing_source_id
        && same_geography
        && same_shipping
        && within_authored_updated
        && *pricing == decided.pricing
        && *sale_observation == decided.sale_observation
        && *availability == decided.availability
        && *lifecycle == decided.lifecycle
        && *auction == decided.auction
        && *created == decided.created
        && *product_title == decided.product_title
        && *product_description == decided.product_description
        && *descriptions == decided.descriptions
        && *image == decided.image
        && *images == decided.images;
    if !same_decision_facts {
        MatchEvidenceChange::Decision
    } else if candidate_title(current) != candidate_title(decided) {
        MatchEvidenceChange::ClassifierTitle
    } else {
        MatchEvidenceChange::Unchanged
    }
}

fn is_withdrawn_candidate(lifecycle: ListingLifecycle) -> bool {
    lifecycle == ListingLifecycle::Withdrawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_match_evaluator::tests::product;
    use domain_primitives::event_id::EventId;
    use domain_primitives::query::range_query::RangeQuery;
    use localization::{Language, Localized};
    use money::Currency;
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::product_listing_image::ProductListingImage;

    fn change(
        apply: impl FnOnce(&mut ProductListingSearchFilterMatchSource),
    ) -> MatchEvidenceChange {
        change_for(
            &ProductListingSearch::new(Language::En, Currency::Eur),
            apply,
        )
    }

    fn change_for(
        search: &ProductListingSearch,
        apply: impl FnOnce(&mut ProductListingSearchFilterMatchSource),
    ) -> MatchEvidenceChange {
        let decided = product().unwrap();
        let mut current = decided.clone();
        current.event_id = EventId::new();
        current.current_event_id = current.event_id;
        current.projection_version += 1;
        current.updated += time::Duration::minutes(20);
        apply(&mut current);
        match_evidence_change(search, &decided, &current)
    }

    #[test]
    fn should_keep_the_evidence_when_only_the_embedding_and_freshness_change() {
        assert_eq!(
            MatchEvidenceChange::Unchanged,
            change(|source| source.embedding = Some(vec![0.1; 4]))
        );
    }

    #[test]
    fn should_keep_the_evidence_when_a_translation_other_than_english_lands() {
        assert_eq!(
            MatchEvidenceChange::Unchanged,
            change(|source| {
                source.titles.insert(Language::De, "Tisch".into());
            })
        );
    }

    #[test]
    fn should_report_the_classifier_title_when_the_english_title_lands() {
        assert_eq!(
            MatchEvidenceChange::ClassifierTitle,
            change(|source| {
                source.titles.insert(Language::En, "Table".into());
            })
        );
    }

    #[test]
    fn should_report_a_decision_change_when_a_decided_fact_changes() {
        let changes: [fn(&mut ProductListingSearchFilterMatchSource); 4] = [
            |source| source.availability = Some(ListingAvailability::Reserved),
            |source| source.product_title = Some(Localized::new(Language::En, "Chair".into())),
            |source| {
                source.images.insert(ProductListingImage::new(
                    url::Url::parse("https://example.test/new.jpg").unwrap(),
                ));
            },
            |source| source.created += time::Duration::seconds(1),
        ];
        for apply in changes {
            assert_eq!(MatchEvidenceChange::Decision, change(apply));
        }
    }

    fn shipping_search() -> ProductListingSearch {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        search.shipping = Some(
            product_listing_core::shipping_search::ShippingSearch::new(
                geo::reference::ReferenceRelease::CURRENT,
                Some(geo::CountryCode::DEU),
                None,
                None,
                false,
            )
            .unwrap(),
        );
        search
    }

    fn reproject_shipping(source: &mut ProductListingSearchFilterMatchSource) {
        source.shipping.prohibited = true;
        source.shipping.conditional = false;
    }

    #[test]
    fn should_ignore_shipping_enrichment_for_a_filter_without_a_shipping_predicate() {
        assert_eq!(MatchEvidenceChange::Unchanged, change(reproject_shipping));
    }

    #[test]
    fn should_report_a_decision_change_when_shipping_changes_under_a_shipping_predicate() {
        assert_eq!(
            MatchEvidenceChange::Decision,
            change_for(&shipping_search(), reproject_shipping)
        );
    }

    #[test]
    fn should_report_a_decision_change_when_freshness_passes_an_authored_maximum() {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        search.updated_query = Some(RangeQuery {
            min: None,
            max: Some(time::OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(10)),
        });
        assert_eq!(
            MatchEvidenceChange::Decision,
            change_for(&search, |source| source.embedding = Some(vec![0.1; 4]))
        );
    }
}
