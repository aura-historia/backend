//! How much of a product-schema review's page set the review's stored schema matrix covers: the
//! coverage the review console shows and approval checks.

use crate::review::model::{
    CrawlerReviewPage, SchemaMatrix, SchemaPageEvaluation, SchemaPageReference,
};
use crate::review::schema_evaluation::schema_matrix_covers_page;
use serde::Serialize;
use std::fmt;

/// Whether an approval goes ahead when the candidate's schema matrix does not cover every review
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteCoverage {
    /// Refuse the approval, so a candidate does not go live on pages nobody saw it extract.
    Refuse,
    /// Approve anyway: the approver confirmed the covered and total page counts.
    Acknowledged,
}

/// The review pages at least one candidate schema applies to, out of all the review's pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReviewPageCoverage {
    pub covered: usize,
    pub total: usize,
    /// The review pages the matrix did not evaluate, which count as not covered: pages that
    /// joined the review while the matrix was evaluated, or every page when no matrix is stored.
    pub not_evaluated: usize,
}

impl ReviewPageCoverage {
    /// The coverage of the review's `pages` by its stored `matrix`, `None` when it stores none
    /// that decodes. Only the review's current pages count, each once.
    pub fn of(matrix: Option<&SchemaMatrix>, pages: &[CrawlerReviewPage]) -> Self {
        let total = pages.len();
        let Some(matrix) = matrix else {
            return Self {
                covered: 0,
                total,
                not_evaluated: total,
            };
        };
        // Every candidate evaluates the same pages.
        let evaluated_pages = matrix
            .candidates
            .first()
            .map_or(&[][..], |candidate| candidate.pages.as_slice());
        let mut coverage = Self {
            covered: 0,
            total,
            not_evaluated: 0,
        };
        for page in pages {
            match evaluated_pages
                .iter()
                .find(|evaluation| evaluates_review_page(evaluation, page))
            {
                None => coverage.not_evaluated += 1,
                Some(evaluation) => {
                    if schema_matrix_covers_page(matrix, &evaluation.page_reference) {
                        coverage.covered += 1;
                    }
                }
            }
        }
        coverage
    }

    /// Every review page is covered. A review without pages shows nothing the candidate extracts,
    /// so its coverage is never complete.
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.covered == self.total
    }

    /// Whether an approval of a candidate with this coverage goes ahead.
    pub fn admits(&self, incomplete_coverage: IncompleteCoverage) -> bool {
        self.is_complete() || incomplete_coverage == IncompleteCoverage::Acknowledged
    }
}

impl fmt::Display for ReviewPageCoverage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} / {} review pages covered by at least one schema",
            self.covered, self.total
        )?;
        if self.not_evaluated > 0 {
            write!(formatter, "; {} not evaluated", self.not_evaluated)?;
        }
        Ok(())
    }
}

/// A live matrix names a review page by its id. A generated matrix refers to the pages it was
/// generated from by position, and those are stored as the review's pages under the same URL.
fn evaluates_review_page(evaluation: &SchemaPageEvaluation, page: &CrawlerReviewPage) -> bool {
    match &evaluation.page_reference {
        SchemaPageReference::Persisted { review_page_id } => *review_page_id == page.review_page_id,
        SchemaPageReference::Input { .. } => evaluation.url == page.url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_WAITING_PAGE, SchemaReviewPageInput};
    use crate::review::schema_evaluation::{
        evaluate_schema_matrix_for_inputs, evaluate_schema_matrix_for_live_review_pages,
    };
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::{CrawlerReviewId, CrawlerReviewPageId};
    use std::collections::HashMap;

    fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
        ExtractionRule {
            selector: selector.into(),
            additional_selectors: Vec::new(),
            extract,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn schema(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(rule("#product-id", ExtractionKind::Text)),
            title: rule(title_selector, ExtractionKind::Text),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: rule("#state", ExtractionKind::Text),
            images: rule("img", ExtractionKind::Attribute { name: "src".into() }),
            raw_attributes: Default::default(),
        }
    }

    /// A product page whose title sits in a `title_tag` element.
    fn input_page(slug: &str, title_tag: &str) -> SchemaReviewPageInput {
        SchemaReviewPageInput {
            url: format!("https://shop.example/products/{slug}"),
            role: PAGE_ROLE_PRIMARY.to_owned(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">{slug}</span><{title_tag}>{slug}</{title_tag}><span id=\"state\">Available</span><img src=\"{slug}.jpg\"></body></html>"
            ),
        }
    }

    fn review_page(page: &SchemaReviewPageInput) -> CrawlerReviewPage {
        CrawlerReviewPage {
            review_page_id: CrawlerReviewPageId::new(),
            review_id: CrawlerReviewId::new(),
            url: page.url.clone(),
            role: page.role.clone(),
            html_hash: String::new(),
            fetched: time::OffsetDateTime::now_utc(),
        }
    }

    /// The live matrix of `schemas` on the review pages it evaluated, each with its HTML.
    fn live_matrix(
        schemas: &[ProductCssSelectorSchema],
        evaluated: &[(&CrawlerReviewPage, &SchemaReviewPageInput)],
    ) -> SchemaMatrix {
        let pages: Vec<(CrawlerReviewPage, String)> = evaluated
            .iter()
            .map(|(page, input)| ((*page).clone(), input.raw_html.clone()))
            .collect();
        evaluate_schema_matrix_for_live_review_pages(
            CrawlerReviewId::new(),
            schemas,
            &[],
            &pages,
            &HashMap::new(),
        )
    }

    fn counts(covered: usize, total: usize, not_evaluated: usize) -> ReviewPageCoverage {
        ReviewPageCoverage {
            covered,
            total,
            not_evaluated,
        }
    }

    #[test]
    fn should_be_complete_when_a_schema_applies_to_every_review_page() {
        let (chair, table) = (input_page("chair", "h1"), input_page("table", "h2"));
        let pages = [review_page(&chair), review_page(&table)];
        let matrix = live_matrix(
            &[schema("h1"), schema("h2")],
            &[(&pages[0], &chair), (&pages[1], &table)],
        );

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(2, 2, 0));
        assert!(coverage.admits(IncompleteCoverage::Refuse));
    }

    #[test]
    fn should_admit_a_page_no_schema_applies_to_only_when_acknowledged() {
        let (chair, table) = (input_page("chair", "h1"), input_page("table", "h2"));
        let pages = [review_page(&chair), review_page(&table)];
        let matrix = live_matrix(&[schema("h1")], &[(&pages[0], &chair), (&pages[1], &table)]);

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(1, 2, 0));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
        assert!(coverage.admits(IncompleteCoverage::Acknowledged));
    }

    /// A page that joined while the matrix was evaluated is not in it, and only the review's
    /// current pages count, so an evaluated page that left the review covers nothing.
    #[test]
    fn should_count_a_review_page_the_matrix_did_not_evaluate_as_not_covered() {
        let (chair, gone, waiting) = (
            input_page("chair", "h1"),
            input_page("gone", "h1"),
            input_page("waiting", "h1"),
        );
        let evaluated = [review_page(&chair), review_page(&gone)];
        let matrix = live_matrix(
            &[schema("h1")],
            &[(&evaluated[0], &chair), (&evaluated[1], &gone)],
        );
        let mut joined = review_page(&waiting);
        joined.role = PAGE_ROLE_WAITING_PAGE.to_owned();

        let coverage = ReviewPageCoverage::of(Some(&matrix), &[evaluated[0].clone(), joined]);

        assert_eq!(coverage, counts(1, 2, 1));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
        assert_eq!(
            coverage.to_string(),
            "1 / 2 review pages covered by at least one schema; 1 not evaluated"
        );
    }

    /// No stored matrix, or one that no longer decodes, vouches for no page.
    #[test]
    fn should_count_every_review_page_as_not_evaluated_without_a_matrix() {
        let pages = [
            review_page(&input_page("chair", "h1")),
            review_page(&input_page("table", "h1")),
        ];

        let coverage = ReviewPageCoverage::of(None, &pages);

        assert_eq!(coverage, counts(0, 2, 2));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
    }

    #[test]
    fn should_match_the_pages_of_a_generated_matrix_by_url() {
        let inputs = [input_page("chair", "h1"), input_page("table", "h2")];
        let matrix = evaluate_schema_matrix_for_inputs(&[schema("h1")], &inputs);
        let pages = inputs.each_ref().map(review_page);

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(1, 2, 0));
        assert_eq!(
            coverage.to_string(),
            "1 / 2 review pages covered by at least one schema"
        );
    }

    #[test]
    fn should_never_be_complete_without_review_pages() {
        let matrix = live_matrix(&[schema("h1")], &[]);

        let coverage = ReviewPageCoverage::of(Some(&matrix), &[]);

        assert_eq!(coverage, counts(0, 0, 0));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
    }
}
