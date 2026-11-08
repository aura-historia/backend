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
use product_listing_normalization::{ListingAvailabilityQuickCheck, quick_check_availability};
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

/// What the ProductListing normalization makes of an extracted `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StateReading {
    /// What the whole `state` resolves to, as the scraper does at runtime and the only value that
    /// becomes the listing's availability; `None` for a value that is no whole availability label.
    pub resolved: Option<ListingAvailability>,
    /// Sold, out of stock or unavailable, named by a window of one to four words of a `state` that
    /// does not resolve as a whole, such as a sentence announcing the sale.
    pub mentioned: Option<ListingAvailability>,
}

impl StateReading {
    /// One reading for the approval guard and the matrix view, so they agree.
    pub(crate) fn of(state: &str) -> Self {
        let resolved = resolved_availability(state);
        let mentioned = match resolved {
            Some(_) => None,
            None => mentioned_unsold_status(state),
        };
        Self {
            resolved,
            mentioned,
        }
    }

    /// Whether the sold-page check counts the `state` as sold or unavailable: it resolves to such
    /// an availability or mentions one.
    pub(crate) fn shows_sold_or_unavailable(self) -> bool {
        self.mentioned.is_some() || is_sold_or_unavailable(self.resolved)
    }
}

fn resolved_availability(state: &str) -> Option<ListingAvailability> {
    quick_check_availability(state)
        .ok()
        .and_then(ListingAvailabilityQuickCheck::availability)
}

/// The most words of a status label the normalization's tables hold, such as "out of stock".
const MAX_STATUS_WORDS: usize = 4;

fn mentioned_unsold_status(state: &str) -> Option<ListingAvailability> {
    let words: Vec<&str> = state
        .split_whitespace()
        .map(|word| word.trim_matches(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect();
    (1..=MAX_STATUS_WORDS.min(words.len()))
        .flat_map(|size| words.windows(size))
        .map(|window| resolved_availability(&window.join(" ")))
        .find(|availability| is_sold_or_unavailable(*availability))
        .flatten()
}

fn is_sold_or_unavailable(availability: Option<ListingAvailability>) -> bool {
    matches!(
        availability,
        Some(
            ListingAvailability::SoldOut
                | ListingAvailability::OutOfStock
                | ListingAvailability::Unavailable
        )
    )
}

/// Whether a candidate's `state` rule resolves to sold, out of stock or unavailable on one of the
/// pages it applies to: a status slot, which shows an amount while the item is for sale.
pub(crate) fn candidate_has_sold_state(pages: &[SchemaPageEvaluation]) -> bool {
    pages
        .iter()
        .filter(|page| page.apply_ok)
        .filter_map(|page| page.extracted.as_ref())
        .any(|extracted| StateReading::of(&extracted.state).shows_sold_or_unavailable())
}

/// The `state` each candidate schema extracts on `page`, for the schemas that apply to it.
fn applied_states<'a>(
    matrix: &'a SchemaMatrix,
    page: &CrawlerReviewPage,
) -> impl Iterator<Item = &'a str> {
    let reference = SchemaPageReference::Persisted {
        review_page_id: page.review_page_id,
    };
    matrix
        .candidates
        .iter()
        .flat_map(|candidate| &candidate.pages)
        .filter(move |evaluation| evaluation.page_reference == reference && evaluation.apply_ok)
        .filter_map(|evaluation| evaluation.extracted.as_ref())
        .map(|extracted| extracted.state.as_str())
}

/// Whether a review page shows a sold or unavailable product: a template captures it as removed,
/// or a candidate schema extracts a `state` that the ProductListing normalization resolves to sold
/// out, out of stock or unavailable, as a `schema.org/SoldOut` read from JSON-LD or the label
/// "Sold" does, or a sentence that mentions such a status. A `state` that names none, such as an
/// add-to-cart button's text, says nothing about the page.
pub(crate) fn shows_sold_or_unavailable_page(
    matrix: &SchemaMatrix,
    coverages: &[(&CrawlerReviewPage, PageCoverage)],
) -> bool {
    coverages.iter().any(|(page, coverage)| {
        *coverage == PageCoverage::CapturedAsRemoved
            || applied_states(matrix, page)
                .any(|state| StateReading::of(state).shows_sold_or_unavailable())
    })
}

/// A `state` a candidate schema extracts on a review page, with what it resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeenState {
    pub url: String,
    pub state: String,
    /// The availability's catalogue code the whole `state` resolves to, `None` when it does not.
    pub resolves_to: Option<&'static str>,
    /// The sold, out of stock or unavailable code a word window of an unresolved `state` names.
    pub mentions: Option<&'static str>,
}

/// The states the candidate schemas extract on the review pages they apply to, once per page and
/// value, as `NO_SOLD_PAGE_TESTED` names them.
pub(crate) fn states_seen(
    matrix: &SchemaMatrix,
    coverages: &[(&CrawlerReviewPage, PageCoverage)],
) -> Vec<SeenState> {
    let mut seen: Vec<SeenState> = Vec::new();
    for (page, _) in coverages {
        for state in applied_states(matrix, page) {
            let reading = StateReading::of(state);
            let entry = SeenState {
                url: page.url.clone(),
                state: shortened(state),
                resolves_to: reading.resolved.map(ListingAvailability::as_str),
                mentions: reading.mentioned.map(ListingAvailability::as_str),
            };
            if !seen.contains(&entry) {
                seen.push(entry);
            }
        }
    }
    seen
}

const SEEN_STATE_MAX_CHARS: usize = 80;

fn shortened(state: &str) -> String {
    let mut chars = state.trim().chars();
    let kept: String = chars.by_ref().take(SEEN_STATE_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}
