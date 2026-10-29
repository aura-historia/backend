//! How much of a product-schema review's page set the review's stored schema matrix covers: the
//! coverage the review console shows and approval checks.

use crate::review::model::{
    CrawlerReviewPage, RemovedPageTemplateOutcome, SchemaMatrix, SchemaPageEvaluation,
    SchemaPageReference,
};
use crate::review::schema_evaluation::schema_matrix_covers_page;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Whether an approval goes ahead when the candidate's schema matrix does not cover every review
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteCoverage {
    /// Refuse the approval, so a candidate does not go live on pages nobody saw it resolve.
    Refuse,
    /// Approve anyway while the review's coverage is still this one, the coverage the approver
    /// confirmed, so a page that joined or a matrix stored since the confirmation is not approved
    /// under it.
    Acknowledged(ReviewPageCoverage),
}

/// The review pages the candidate resolves, out of all the review's pages: a page is covered when
/// the candidate's removed-page templates capture it as removed, or else when at least one
/// candidate schema applies to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPageCoverage {
    pub covered: usize,
    /// The covered pages the candidate's removed-page templates capture as removed.
    pub removed: usize,
    pub total: usize,
    /// The review pages the matrix did not evaluate, which count as not covered: pages that
    /// joined the review while the matrix was evaluated, or every page when no matrix is stored.
    pub not_evaluated: usize,
}

impl ReviewPageCoverage {
    /// The coverage of the review's `pages` by its stored `matrix`, `None` when it stores none
    /// that decodes. Only the review's current pages count, each once.
    pub fn of(matrix: Option<&SchemaMatrix>, pages: &[CrawlerReviewPage]) -> Self {
        let mut coverage = Self {
            covered: 0,
            removed: 0,
            total: pages.len(),
            not_evaluated: 0,
        };
        for (_, page_coverage) in page_coverages(matrix, pages) {
            match page_coverage {
                PageCoverage::NotEvaluated => coverage.not_evaluated += 1,
                PageCoverage::CapturedAsRemoved => {
                    coverage.covered += 1;
                    coverage.removed += 1;
                }
                PageCoverage::CoveredBySchema => coverage.covered += 1,
                PageCoverage::NotCovered => {}
            }
        }
        coverage
    }

    /// Every review page is covered. A review without pages shows nothing the candidate extracts,
    /// so its coverage is never complete.
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.covered == self.total
    }

    /// Whether an approval of a candidate with this coverage goes ahead: always when it is
    /// complete, otherwise only when the approver acknowledged exactly this coverage.
    pub fn admits(&self, incomplete_coverage: IncompleteCoverage) -> bool {
        self.is_complete() || incomplete_coverage == IncompleteCoverage::Acknowledged(*self)
    }
}

impl fmt::Display for ReviewPageCoverage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} / {} review pages covered",
            self.covered, self.total
        )?;
        if self.removed > 0 {
            // An acknowledged coverage is the client's, so its counts need not add up.
            write!(
                formatter,
                " ({} by a schema, {} captured as removed)",
                self.covered.saturating_sub(self.removed),
                self.removed
            )?;
        } else {
            write!(formatter, " by at least one schema")?;
        }
        if self.not_evaluated > 0 {
            write!(formatter, "; {} not evaluated", self.not_evaluated)?;
        }
        Ok(())
    }
}

/// What a review's stored schema matrix shows for one of its pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCoverage {
    /// The matrix did not evaluate the page: it joined while the matrix was evaluated, or no
    /// matrix is stored.
    NotEvaluated,
    /// The candidate's removed-page templates capture the page as removed.
    CapturedAsRemoved,
    /// At least one candidate schema applies to the page.
    CoveredBySchema,
    /// The matrix evaluated the page and the candidate resolves it neither way.
    NotCovered,
}

/// Each of the review's `pages` with what its stored `matrix` shows for it, in page order;
/// every page is [`PageCoverage::NotEvaluated`] without a matrix.
pub fn page_coverages<'a>(
    matrix: Option<&SchemaMatrix>,
    pages: &'a [CrawlerReviewPage],
) -> Vec<(&'a CrawlerReviewPage, PageCoverage)> {
    pages
        .iter()
        .map(|page| (page, page_coverage(matrix, page)))
        .collect()
}

fn page_coverage(matrix: Option<&SchemaMatrix>, page: &CrawlerReviewPage) -> PageCoverage {
    let Some(matrix) = matrix else {
        return PageCoverage::NotEvaluated;
    };
    // Every candidate evaluates the same pages.
    let evaluated_pages = matrix
        .candidates
        .first()
        .map_or(&[][..], |candidate| candidate.pages.as_slice());
    let Some(evaluation) = evaluated_pages
        .iter()
        .find(|evaluation| evaluates_review_page(evaluation, page))
    else {
        return PageCoverage::NotEvaluated;
    };
    if captured_as_removed(matrix, &evaluation.page_reference) {
        PageCoverage::CapturedAsRemoved
    } else if schema_matrix_covers_page(matrix, &evaluation.page_reference) {
        PageCoverage::CoveredBySchema
    } else {
        PageCoverage::NotCovered
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

/// The candidate's removed-page templates match the page and no candidate schema still extracts
/// a product from it, so the scraper's removal guard captures it as removed once the candidate is
/// live. A page a template matches but a candidate schema still extracts is left to the schemas.
fn captured_as_removed(matrix: &SchemaMatrix, page_reference: &SchemaPageReference) -> bool {
    matrix.removed_page_matches.iter().any(|matched| {
        matched.page_reference == *page_reference
            && matched.outcome == RemovedPageTemplateOutcome::Removed
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_WAITING_PAGE, SchemaReviewPageInput};
    use crate::review::schema_evaluation::{
        evaluate_schema_matrix_for_inputs, evaluate_schema_matrix_for_live_review_pages,
    };
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
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

    fn notice_template(selector: &str, text: &str) -> RemovedPageSchema {
        RemovedPageSchema {
            selector: selector.into(),
            text: Some(text.to_owned()),
            regex: None,
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

    /// A page the shop serves with a removal notice and no product markup.
    fn notice_page(slug: &str) -> SchemaReviewPageInput {
        SchemaReviewPageInput {
            url: format!("https://shop.example/products/{slug}"),
            role: PAGE_ROLE_WAITING_PAGE.to_owned(),
            raw_html:
                "<html><body><p class=\"notice\">This item is no longer available</p></body></html>"
                    .to_owned(),
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

    /// The live matrix of `schemas` and removed-page `templates` on the review pages it
    /// evaluated, each with its HTML.
    fn live_matrix(
        schemas: &[ProductCssSelectorSchema],
        templates: &[RemovedPageSchema],
        evaluated: &[(&CrawlerReviewPage, &SchemaReviewPageInput)],
    ) -> SchemaMatrix {
        let pages: Vec<(CrawlerReviewPage, String)> = evaluated
            .iter()
            .map(|(page, input)| ((*page).clone(), input.raw_html.clone()))
            .collect();
        evaluate_schema_matrix_for_live_review_pages(
            CrawlerReviewId::new(),
            schemas,
            templates,
            &pages,
            &[],
            &HashMap::new(),
        )
    }

    fn counts(
        covered: usize,
        removed: usize,
        total: usize,
        not_evaluated: usize,
    ) -> ReviewPageCoverage {
        ReviewPageCoverage {
            covered,
            removed,
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
            &[],
            &[(&pages[0], &chair), (&pages[1], &table)],
        );

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(2, 0, 2, 0));
        assert!(coverage.admits(IncompleteCoverage::Refuse));
    }

    /// The acknowledgement names the coverage the approver confirmed: once a page joined or a
    /// matrix was stored since, the review's coverage differs and the acknowledgement admits
    /// nothing.
    #[test]
    fn should_admit_a_page_no_schema_applies_to_only_when_its_current_coverage_is_acknowledged() {
        let (chair, table) = (input_page("chair", "h1"), input_page("table", "h2"));
        let pages = [review_page(&chair), review_page(&table)];
        let matrix = live_matrix(
            &[schema("h1")],
            &[],
            &[(&pages[0], &chair), (&pages[1], &table)],
        );

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(1, 0, 2, 0));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
        assert!(coverage.admits(IncompleteCoverage::Acknowledged(counts(1, 0, 2, 0))));
        for stale in [
            counts(1, 0, 3, 1),
            counts(0, 0, 2, 2),
            counts(1, 1, 2, 0),
            counts(2, 0, 2, 0),
        ] {
            assert!(
                !coverage.admits(IncompleteCoverage::Acknowledged(stale)),
                "{stale:?}"
            );
        }
    }

    /// A refusal names the acknowledged coverage a client sent, whose counts need not add up.
    #[test]
    fn should_describe_an_acknowledged_coverage_whose_counts_do_not_add_up() {
        assert_eq!(
            counts(0, 1, 2, 0).to_string(),
            "0 / 2 review pages covered (0 by a schema, 1 captured as removed)"
        );
    }

    /// Declaring a page removed resolves it only where the stored matrix shows the candidate's
    /// templates capture it as removed; a template that also matches a page a schema still
    /// extracts leaves that page to the schemas.
    #[test]
    fn should_count_a_page_the_removed_page_templates_capture_as_removed_as_covered() {
        let (chair, sold) = (input_page("chair", "h1"), notice_page("sold"));
        let pages = [review_page(&chair), review_page(&sold)];
        let evaluated = [(&pages[0], &chair), (&pages[1], &sold)];

        let declared = ReviewPageCoverage::of(
            Some(&live_matrix(
                &[schema("h1")],
                &[notice_template(".notice", "no longer available")],
                &evaluated,
            )),
            &pages,
        );
        let over_broad = ReviewPageCoverage::of(
            Some(&live_matrix(
                &[schema("h1")],
                &[notice_template("body", "available")],
                &evaluated,
            )),
            &pages,
        );
        let undeclared =
            ReviewPageCoverage::of(Some(&live_matrix(&[schema("h1")], &[], &evaluated)), &pages);

        assert_eq!(declared, counts(2, 1, 2, 0));
        assert!(declared.admits(IncompleteCoverage::Refuse));
        assert_eq!(
            declared.to_string(),
            "2 / 2 review pages covered (1 by a schema, 1 captured as removed)"
        );
        assert_eq!(over_broad, counts(2, 1, 2, 0));
        assert_eq!(undeclared, counts(1, 0, 2, 0));
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
            &[],
            &[(&evaluated[0], &chair), (&evaluated[1], &gone)],
        );
        let mut joined = review_page(&waiting);
        joined.role = PAGE_ROLE_WAITING_PAGE.to_owned();

        let coverage = ReviewPageCoverage::of(Some(&matrix), &[evaluated[0].clone(), joined]);

        assert_eq!(coverage, counts(1, 0, 2, 1));
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

        assert_eq!(coverage, counts(0, 0, 2, 2));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
    }

    #[test]
    fn should_match_the_pages_of_a_generated_matrix_by_url() {
        let inputs = [input_page("chair", "h1"), input_page("table", "h2")];
        let matrix = evaluate_schema_matrix_for_inputs(&[schema("h1")], &inputs);
        let pages = inputs.each_ref().map(review_page);

        let coverage = ReviewPageCoverage::of(Some(&matrix), &pages);

        assert_eq!(coverage, counts(1, 0, 2, 0));
        assert_eq!(
            coverage.to_string(),
            "1 / 2 review pages covered by at least one schema"
        );
    }

    #[test]
    fn should_never_be_complete_without_review_pages() {
        let matrix = live_matrix(&[schema("h1")], &[], &[]);

        let coverage = ReviewPageCoverage::of(Some(&matrix), &[]);

        assert_eq!(coverage, counts(0, 0, 0, 0));
        assert!(!coverage.admits(IncompleteCoverage::Refuse));
    }
}
