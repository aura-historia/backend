//! The approval guard's checks of what the review pages show against what the candidate captures:
//! a `price` or `description` rule the pages call for and a sold or unavailable page among them
//! (`docs/crawler/extraction-and-review.md`, approval guard). They read the matrix's page signals
//! and extractions, never the pages themselves.

use crate::review::approval_guard::GuardedPage;
use crate::review::model::{
    CrawlerReviewPage, PageSignals, SchemaMatrix, SchemaPageEvaluation, SchemaPageReference,
};
use crate::review::page_coverage::PageCoverage;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_normalization::quick_check_availability;
use serde::Serialize;

/// An optional field whose rule no schema applying to the pages has, though the pages show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingContentRule {
    pub field: &'static str,
    pub pages: Vec<GuardedPage>,
}

/// The pages a matrix evaluated that carry no signals, which a matrix stored before the signals
/// does for all of them.
pub(crate) fn pages_without_signals(matrix: &SchemaMatrix) -> bool {
    matrix
        .candidates
        .iter()
        .flat_map(|candidate| &candidate.pages)
        .any(|page| signals_of(matrix, &page.page_reference).is_none())
}

fn signals_of<'a>(
    matrix: &'a SchemaMatrix,
    page_reference: &SchemaPageReference,
) -> Option<&'a PageSignals> {
    matrix
        .page_signals
        .iter()
        .find(|signals| signals.page_reference == *page_reference)
}

/// The `price` and `description` rules missing for review pages that show a price or a
/// description block: on a page a candidate schema applies to, none of the schemas that apply has
/// the rule. An estimate rule captures the amount a price element shows too, as an auction-style
/// shop shows its estimate there instead of a price. A page no schema applies to, or a template
/// captures as removed, is left to the coverage checks, and so is a page the matrix did not
/// evaluate.
pub(crate) fn missing_content_rules(
    matrix: &SchemaMatrix,
    coverages: &[(&CrawlerReviewPage, PageCoverage)],
) -> Vec<MissingContentRule> {
    let mut price = Vec::new();
    let mut description = Vec::new();
    for (page, coverage) in coverages {
        if *coverage != PageCoverage::CoveredBySchema {
            continue;
        }
        let reference = SchemaPageReference::Persisted {
            review_page_id: page.review_page_id,
        };
        let Some(signals) = signals_of(matrix, &reference) else {
            continue;
        };
        let applying: Vec<&SchemaPageEvaluation> = matrix
            .candidates
            .iter()
            .filter_map(|candidate| {
                candidate
                    .pages
                    .iter()
                    .find(|evaluation| evaluation.page_reference == reference)
            })
            .filter(|evaluation| evaluation.apply_ok)
            .collect();
        let guarded = || GuardedPage {
            review_page_id: page.review_page_id,
            url: page.url.clone(),
        };
        if signals.shows_price_text && !any_has_rule(&applying, &PRICE_RULES) {
            price.push(guarded());
        }
        if signals.shows_description_block && !any_has_rule(&applying, &["description"]) {
            description.push(guarded());
        }
    }
    [("price", price), ("description", description)]
        .into_iter()
        .filter(|(_, pages)| !pages.is_empty())
        .map(|(field, pages)| MissingContentRule { field, pages })
        .collect()
}

/// The rules that capture an amount a price element shows.
const PRICE_RULES: [&str; 3] = ["price", "price_estimate_min", "price_estimate_max"];

/// Whether a schema of `evaluations` has a rule for one of `fields`.
fn any_has_rule(evaluations: &[&SchemaPageEvaluation], fields: &[&str]) -> bool {
    evaluations.iter().any(|evaluation| {
        evaluation
            .fields
            .iter()
            .any(|rule| fields.contains(&rule.field.as_str()))
    })
}

/// Whether a review page shows a sold or unavailable product: a template captures it as removed,
/// or a candidate schema extracts a `state` that the ProductListing normalization resolves to sold
/// out, out of stock or unavailable, as a `schema.org/SoldOut` read from JSON-LD or the label
/// "Sold" does. A `state` the normalization does not recognize, such as an add-to-cart button's
/// text, says nothing about the page.
pub(crate) fn shows_sold_or_unavailable_page(
    matrix: &SchemaMatrix,
    coverages: &[(&CrawlerReviewPage, PageCoverage)],
) -> bool {
    coverages.iter().any(|(page, coverage)| {
        let reference = SchemaPageReference::Persisted {
            review_page_id: page.review_page_id,
        };
        *coverage == PageCoverage::CapturedAsRemoved
            || matrix
                .candidates
                .iter()
                .flat_map(|candidate| &candidate.pages)
                .filter(|evaluation| evaluation.page_reference == reference && evaluation.apply_ok)
                .filter_map(|evaluation| evaluation.extracted.as_ref())
                .any(|extracted| state_is_sold_or_unavailable(&extracted.state))
    })
}

fn state_is_sold_or_unavailable(state: &str) -> bool {
    matches!(
        quick_check_availability(state)
            .ok()
            .and_then(|check| check.availability()),
        Some(
            ListingAvailability::SoldOut
                | ListingAvailability::OutOfStock
                | ListingAvailability::Unavailable
        )
    )
}
