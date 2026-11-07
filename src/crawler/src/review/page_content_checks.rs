//! The approval guard's checks of what the review pages show against what the candidate captures:
//! a `price` or `description` rule the pages call for and a sold or unavailable page among them
//! (`docs/crawler/extraction-and-review.md`, approval guard). They read the matrix's page signals
//! and extractions, never the pages themselves.

use crate::review::approval_guard::GuardedPage;
use crate::review::model::{
    CrawlerReviewPage, PageSignals, SchemaMatrix, SchemaPageEvaluation, SchemaPageReference,
};
use crate::review::page_coverage::PageCoverage;
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
/// the rule. A page no schema applies to, or a template captures as removed, is left to the
/// coverage checks, and so is a page the matrix did not evaluate.
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
        if signals.shows_price_text && !any_has_rule(&applying, "price") {
            price.push(guarded());
        }
        if signals.shows_description_block && !any_has_rule(&applying, "description") {
            description.push(guarded());
        }
    }
    [("price", price), ("description", description)]
        .into_iter()
        .filter(|(_, pages)| !pages.is_empty())
        .map(|(field, pages)| MissingContentRule { field, pages })
        .collect()
}

fn any_has_rule(evaluations: &[&SchemaPageEvaluation], field: &str) -> bool {
    evaluations
        .iter()
        .any(|evaluation| evaluation.fields.iter().any(|rule| rule.field == field))
}
