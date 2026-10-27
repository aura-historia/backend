//! The deterministic checks an approval by a review agent must pass in `agent` authoring mode. A
//! check reads only what the server itself evaluated and stored, never a matrix or a verdict a
//! client sends; the contract is in `docs/crawler/extraction-and-review.md` (approval guard).

use crate::CrawlerReviewPageId;
use crate::review::model::{
    CrawlerReviewPage, LiveMatrixEvaluation, SchemaMatrix, SchemaPageReference, UrlPatternDecision,
    UrlPatternReviewCandidate, UrlSampleEvaluation,
};
use crate::review::page_coverage::{PageCoverage, page_coverages};
use crate::review::schema_evaluation::{
    ImplausiblePage, OptionalRuleTotalMiss, implausible_pages, optional_rules_missing_on_every_page,
};
use crate::review::url_sample_evidence::{SampleUrl, classified_as_product, is_non_product_url};
use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;
use time::OffsetDateTime;

/// One check a guarded agent approval failed, as the refusal names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "check", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailedApprovalCheck {
    /// The approving agent itself changed the candidate version it approves, or the pages it is
    /// evaluated on: no agent approves its own work.
    ApproverIsAuthor,
    /// The review stores no schema matrix the server evaluated on its live pages.
    LiveMatrixMissing,
    /// The stored live evaluation, a product-schema review's matrix or a URL-pattern review's
    /// sample evaluation, evaluated another candidate version, or another configuration than
    /// approval would make live now: another schema list, other removed-page templates or another
    /// URL-pattern decision; or a page or URL that left the review.
    LiveMatrixOutdated { reason: String },
    /// The stored live evaluation's pages were fetched longer ago than allowed.
    LiveMatrixExpired {
        #[serde(with = "time::serde::rfc3339")]
        fetched_at: OffsetDateTime,
        max_age_minutes: u64,
    },
    /// Review pages the stored live matrix did not evaluate, such as pages that joined since.
    PagesNotEvaluated { pages: Vec<GuardedPage> },
    /// Review pages no candidate schema applies to and no removed-page template captures as
    /// removed.
    PagesNotCovered { pages: Vec<GuardedPage> },
    /// Product pages on which a candidate schema leaves `title` or `state` empty and no other
    /// schema applies.
    RequiredFieldEmpty { fields: Vec<EmptyRequiredField> },
    /// Rules of a candidate schema that match nothing on every page the schema applies to.
    RuleMatchesNothing { rules: Vec<OptionalRuleTotalMiss> },
    /// Candidate extractions that fail a per-page, cross-page or identity agreement plausibility
    /// check, which deterministic approval in `auto` mode refuses too, on any page.
    ImplausibleExtraction { pages: Vec<ImplausiblePage> },
    /// The URL-pattern candidate decides nothing yet.
    CandidateUndecided,
    /// The review's URL sample holds no URL known as a product page: none the crawler captured a
    /// product from, and none whose page showed one schema.org `Product` in the stored live
    /// evaluation of the sample. Nothing then shows that the decision finds products: the agent
    /// evaluates the sample live, or a human decides, as on a new domain without product markup.
    NoCapturedProductUrl,
    /// Sample URLs known as product pages, captured or by their markup, that the decision does
    /// not classify as products.
    ProductUrlsNotMatched { urls: Vec<String> },
    /// Sample URLs the crawler classifies as category, imprint or info pages that the pattern
    /// matches.
    NonProductUrlsMatched { urls: Vec<String> },
}

/// A review page a check names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuardedPage {
    pub review_page_id: CrawlerReviewPageId,
    pub url: String,
}

/// A required field a candidate schema leaves empty on a product page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmptyRequiredField {
    pub review_page_id: CrawlerReviewPageId,
    pub url: String,
    pub schema_index: usize,
    pub field: &'static str,
}

impl fmt::Display for FailedApprovalCheck {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApproverIsAuthor => write!(
                formatter,
                "the approving agent changed the candidate version it approves or its pages; another agent or a human approves it"
            ),
            Self::LiveMatrixMissing => write!(
                formatter,
                "no schema matrix evaluated on the live review pages is stored; evaluate the matrix"
            ),
            Self::LiveMatrixOutdated { reason } => write!(
                formatter,
                "the stored live evaluation is outdated ({reason}); evaluate the matrix again"
            ),
            Self::LiveMatrixExpired {
                max_age_minutes, ..
            } => write!(
                formatter,
                "the stored live evaluation's pages were fetched more than {max_age_minutes} minutes ago; evaluate the matrix again"
            ),
            Self::PagesNotEvaluated { pages } => {
                write!(formatter, "{} review page(s) not evaluated", pages.len())
            }
            Self::PagesNotCovered { pages } => write!(
                formatter,
                "{} review page(s) covered by no schema and captured as removed by no template",
                pages.len()
            ),
            Self::RequiredFieldEmpty { fields } => write!(
                formatter,
                "{} required field(s) empty on product pages",
                fields.len()
            ),
            Self::RuleMatchesNothing { rules } => write!(
                formatter,
                "{} rule(s) match nothing on every page their schema applies to",
                rules.len()
            ),
            Self::ImplausibleExtraction { pages } => write!(
                formatter,
                "{} extraction(s) fail a plausibility check deterministic approval refuses",
                pages.len()
            ),
            Self::CandidateUndecided => write!(formatter, "the URL-pattern candidate is undecided"),
            Self::NoCapturedProductUrl => write!(
                formatter,
                "the review's URL sample holds no URL known as a product page, neither one the crawler captured a product from nor one whose page showed schema.org Product markup in a current live evaluation, so nothing shows the decision finds products; evaluate the sample live, or stage the candidate and leave the decision to a human"
            ),
            Self::ProductUrlsNotMatched { urls } => write!(
                formatter,
                "the pattern misses {} sample URL(s) known as product pages",
                urls.len()
            ),
            Self::NonProductUrlsMatched { urls } => write!(
                formatter,
                "the pattern matches {} sample URL(s) the crawler classifies as category, imprint or info pages",
                urls.len()
            ),
        }
    }
}

/// What the review is now, against which a stored live matrix is current or outdated.
pub(crate) struct CurrentReview<'a> {
    pub candidate_version: i64,
    /// The fingerprint of the product-schema configuration approval would make live now.
    pub configuration_sha256: &'a str,
    pub pages: &'a [CrawlerReviewPage],
}

/// The checks an agent approval of a product-schema review fails, none when it may go ahead: the
/// review's stored `matrix` is a live evaluation of exactly what approval would make live, of
/// exactly the review's current pages, fetched within `max_age` of `now`; every review page is
/// covered by a schema or captured as removed; no schema leaves a required field empty on a
/// product page; no rule matches nothing on every page its schema applies to; and no extraction
/// fails a plausibility check that deterministic approval refuses, so the guard is never less
/// strict than the `auto` gate.
pub(crate) fn product_schema_approval_checks(
    matrix: Option<&SchemaMatrix>,
    review: &CurrentReview<'_>,
    now: OffsetDateTime,
    max_age: Duration,
) -> Vec<FailedApprovalCheck> {
    let Some((matrix, evaluation)) =
        matrix.and_then(|matrix| Some((matrix, matrix.live_evaluation.as_ref()?)))
    else {
        return vec![FailedApprovalCheck::LiveMatrixMissing];
    };
    if let Some(reason) = outdated_reason(matrix, evaluation, review) {
        return vec![FailedApprovalCheck::LiveMatrixOutdated { reason }];
    }
    let mut failed = Vec::new();
    if let Some(expired) = expired_check(evaluation, now, max_age) {
        failed.push(expired);
    }
    let coverages = page_coverages(Some(matrix), review.pages);
    let pages_with = |wanted: PageCoverage| -> Vec<GuardedPage> {
        coverages
            .iter()
            .filter(|(_, coverage)| *coverage == wanted)
            .map(|(page, _)| GuardedPage {
                review_page_id: page.review_page_id,
                url: page.url.clone(),
            })
            .collect()
    };
    let not_evaluated = pages_with(PageCoverage::NotEvaluated);
    if !not_evaluated.is_empty() {
        failed.push(FailedApprovalCheck::PagesNotEvaluated {
            pages: not_evaluated,
        });
    }
    let not_covered = pages_with(PageCoverage::NotCovered);
    let empty_fields = empty_required_fields(matrix, &not_covered);
    if !not_covered.is_empty() {
        failed.push(FailedApprovalCheck::PagesNotCovered { pages: not_covered });
    }
    if !empty_fields.is_empty() {
        failed.push(FailedApprovalCheck::RequiredFieldEmpty {
            fields: empty_fields,
        });
    }
    let rules = optional_rules_missing_on_every_page(matrix);
    if !rules.is_empty() {
        failed.push(FailedApprovalCheck::RuleMatchesNothing { rules });
    }
    let implausible = implausible_pages(matrix);
    if !implausible.is_empty() {
        failed.push(FailedApprovalCheck::ImplausibleExtraction { pages: implausible });
    }
    failed
}

/// Why a stored live matrix no longer describes the review, `None` while it does.
fn outdated_reason(
    matrix: &SchemaMatrix,
    evaluation: &LiveMatrixEvaluation,
    review: &CurrentReview<'_>,
) -> Option<String> {
    if evaluation.candidate_version != review.candidate_version {
        return Some(format!(
            "it evaluated candidate version {}, the review is at {}",
            evaluation.candidate_version, review.candidate_version
        ));
    }
    if evaluation.configuration_sha256 != review.configuration_sha256 {
        return Some(
            "the live schemas or stored removed-page templates approval would merge or rebase onto changed since"
                .to_owned(),
        );
    }
    let evaluates_a_review_page = |reference: &SchemaPageReference| match reference {
        SchemaPageReference::Persisted { review_page_id } => review
            .pages
            .iter()
            .any(|page| page.review_page_id == *review_page_id),
        SchemaPageReference::Input { .. } => false,
    };
    let evaluated_pages = matrix
        .candidates
        .iter()
        .flat_map(|candidate| &candidate.pages);
    (!evaluated_pages
        .map(|page| &page.page_reference)
        .chain(
            matrix
                .removed_page_matches
                .iter()
                .map(|matched| &matched.page_reference),
        )
        .all(evaluates_a_review_page))
    .then(|| "it evaluated a page that is no longer on the review".to_owned())
}

/// The required fields a candidate schema leaves empty on the product pages no schema covers:
/// the schema found the `title` or `state` element, but it holds no text.
fn empty_required_fields(
    matrix: &SchemaMatrix,
    not_covered: &[GuardedPage],
) -> Vec<EmptyRequiredField> {
    let mut fields = Vec::new();
    for page in not_covered {
        let reference = SchemaPageReference::Persisted {
            review_page_id: page.review_page_id,
        };
        for candidate in &matrix.candidates {
            let Some(evaluation) = candidate
                .pages
                .iter()
                .find(|evaluation| evaluation.page_reference == reference)
            else {
                continue;
            };
            for finding in evaluation.plausibility_findings.as_slice() {
                let field = match finding.field {
                    PlausibilityField::Title => "title",
                    PlausibilityField::State => "state",
                    _ => continue,
                };
                if finding.check == PlausibilityCheck::Blank {
                    fields.push(EmptyRequiredField {
                        review_page_id: page.review_page_id,
                        url: page.url.clone(),
                        schema_index: candidate.schema_index,
                        field,
                    });
                }
            }
        }
    }
    fields
}

/// What a URL-pattern review is now, against which its stored live evaluation of the sample is
/// current or outdated, and the sample as the crawler knows it.
pub(crate) struct CurrentUrlPatternReview<'a> {
    pub candidate: &'a UrlPatternReviewCandidate,
    pub candidate_version: i64,
    pub sample: &'a [SampleUrl],
    /// The live evaluation of the sample the review stores, if any.
    pub evaluation: Option<&'a UrlSampleEvaluation>,
}

/// The checks an agent approval of a URL-pattern review fails, none when it may go ahead: the
/// candidate is decided; a stored live evaluation of the sample, which only counts when there is
/// one, evaluated exactly this decision at this candidate version on URLs of the sample, within
/// `max_age` of `now`; the sample holds a URL known as a product page, one the crawler captured a
/// product from or whose page showed one schema.org `Product` in that evaluation; and the
/// decision classifies as products every such URL and none of the other sample URLs the crawler's
/// own URL classification puts in the category, imprint or info class.
pub(crate) fn url_pattern_approval_checks(
    review: &CurrentUrlPatternReview<'_>,
    now: OffsetDateTime,
    max_age: Duration,
) -> Vec<FailedApprovalCheck> {
    let pattern = match &review.candidate.decision {
        UrlPatternDecision::Undecided => return vec![FailedApprovalCheck::CandidateUndecided],
        UrlPatternDecision::NoPattern => None,
        UrlPatternDecision::Pattern { value } => match Regex::new(value) {
            Ok(pattern) => Some(pattern),
            // Approval refuses an invalid pattern on its own.
            Err(_) => return Vec::new(),
        },
    };
    let mut failed = Vec::new();
    let mut product_urls: HashSet<&str> = review
        .sample
        .iter()
        .filter(|url| url.captured_product)
        .map(|url| url.url.as_str())
        .collect();
    if let Some(evaluation) = review.evaluation {
        if let Some(reason) = outdated_sample_reason(evaluation, review) {
            return vec![FailedApprovalCheck::LiveMatrixOutdated { reason }];
        }
        if let Some(expired) = expired_check(&evaluation.live_evaluation, now, max_age) {
            failed.push(expired);
        }
        product_urls.extend(evaluation.product_markup_urls());
    }
    let classified_as_product = |url: &str| classified_as_product(url, pattern.as_ref());
    let products: Vec<&SampleUrl> = review
        .sample
        .iter()
        .filter(|url| product_urls.contains(url.url.as_str()))
        .collect();
    if products.is_empty() {
        failed.push(FailedApprovalCheck::NoCapturedProductUrl);
    }
    let missed: Vec<String> = products
        .iter()
        .filter(|url| !classified_as_product(&url.url))
        .map(|url| url.url.clone())
        .collect();
    if !missed.is_empty() {
        failed.push(FailedApprovalCheck::ProductUrlsNotMatched { urls: missed });
    }
    let matched_non_products: Vec<String> = review
        .sample
        .iter()
        .filter(|url| !product_urls.contains(url.url.as_str()) && is_non_product_url(&url.url))
        .filter(|url| classified_as_product(&url.url))
        .map(|url| url.url.clone())
        .collect();
    if !matched_non_products.is_empty() {
        failed.push(FailedApprovalCheck::NonProductUrlsMatched {
            urls: matched_non_products,
        });
    }
    failed
}

/// Why a stored live evaluation of the sample no longer describes the review, `None` while it
/// does.
fn outdated_sample_reason(
    evaluation: &UrlSampleEvaluation,
    review: &CurrentUrlPatternReview<'_>,
) -> Option<String> {
    let evaluated = &evaluation.live_evaluation;
    if evaluated.candidate_version != review.candidate_version {
        return Some(format!(
            "it evaluated candidate version {}, the review is at {}",
            evaluated.candidate_version, review.candidate_version
        ));
    }
    if evaluated.configuration_sha256 != review.candidate.decision_sha256() {
        return Some("it evaluated another URL-pattern decision".to_owned());
    }
    evaluation
        .urls
        .iter()
        .any(|evaluated| !review.sample.iter().any(|url| url.url == evaluated.url))
        .then(|| "it evaluated a URL that is not in the review's sample".to_owned())
}

/// The refusal of a live evaluation whose pages were fetched more than `max_age` before `now`.
fn expired_check(
    evaluation: &LiveMatrixEvaluation,
    now: OffsetDateTime,
    max_age: Duration,
) -> Option<FailedApprovalCheck> {
    (now - evaluation.fetched_at > max_age).then_some(FailedApprovalCheck::LiveMatrixExpired {
        fetched_at: evaluation.fetched_at,
        max_age_minutes: max_age.as_secs() / 60,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{EvaluatedSampleUrl, PAGE_ROLE_PRIMARY, SampleUrlOutcome};
    use crate::review::schema_evaluation::{
        evaluate_schema_matrix_for_live_review_pages, schema_matrix_deterministic_approval_ok,
    };
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::scraper::plausibility::{CrossPageCheck, IdentityAgreementCheck};
    use crate::{CrawlerReviewId, CrawlerReviewPageId};
    use std::collections::HashMap;

    const CONFIGURATION: &str = "configuration-sha256";
    const MAX_AGE: Duration = Duration::from_secs(30 * 60);

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

    /// A review page and the HTML the shop serves for it: a product whose title sits in a
    /// `title_tag` element holding `title`.
    fn product_page(slug: &str, title_tag: &str, title: &str) -> (CrawlerReviewPage, String) {
        (
            CrawlerReviewPage {
                review_page_id: CrawlerReviewPageId::new(),
                review_id: CrawlerReviewId::new(),
                url: format!("https://shop.example/products/{slug}"),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                html_hash: String::new(),
                fetched: OffsetDateTime::now_utc(),
            },
            format!(
                "<html><body><span id=\"product-id\">{slug}</span><{title_tag}>{title}</{title_tag}><span id=\"state\">Available</span><img src=\"{slug}.jpg\"></body></html>"
            ),
        )
    }

    fn notice_page(slug: &str) -> (CrawlerReviewPage, String) {
        let (page, _) = product_page(slug, "h1", slug);
        (
            page,
            "<html><body><p class=\"notice\">This item is no longer available</p></body></html>"
                .to_owned(),
        )
    }

    /// The live matrix the server stores after evaluating `schemas` and `templates` on `pages`
    /// at candidate version 3, its pages fetched `fetched_ago` before now.
    fn live_matrix(
        schemas: &[ProductCssSelectorSchema],
        templates: &[RemovedPageSchema],
        pages: &[(CrawlerReviewPage, String)],
        fetched_ago: Duration,
    ) -> SchemaMatrix {
        live_matrix_of_captured(schemas, templates, pages, fetched_ago, &HashMap::new())
    }

    /// As [`live_matrix`], with the source listing id each page URL's listing was captured under.
    fn live_matrix_of_captured(
        schemas: &[ProductCssSelectorSchema],
        templates: &[RemovedPageSchema],
        pages: &[(CrawlerReviewPage, String)],
        fetched_ago: Duration,
        captured_identities: &HashMap<String, String>,
    ) -> SchemaMatrix {
        let mut matrix = evaluate_schema_matrix_for_live_review_pages(
            CrawlerReviewId::new(),
            schemas,
            templates,
            pages,
            captured_identities,
        );
        matrix.live_evaluation = Some(LiveMatrixEvaluation {
            candidate_version: 3,
            configuration_sha256: CONFIGURATION.to_owned(),
            fetched_at: OffsetDateTime::now_utc() - fetched_ago,
        });
        matrix
    }

    fn checks(
        matrix: Option<&SchemaMatrix>,
        pages: &[CrawlerReviewPage],
    ) -> Vec<FailedApprovalCheck> {
        checks_of(matrix, pages, 3, CONFIGURATION)
    }

    fn checks_of(
        matrix: Option<&SchemaMatrix>,
        pages: &[CrawlerReviewPage],
        candidate_version: i64,
        configuration_sha256: &str,
    ) -> Vec<FailedApprovalCheck> {
        product_schema_approval_checks(
            matrix,
            &CurrentReview {
                candidate_version,
                configuration_sha256,
                pages,
            },
            OffsetDateTime::now_utc(),
            MAX_AGE,
        )
    }

    fn review_pages(pages: &[(CrawlerReviewPage, String)]) -> Vec<CrawlerReviewPage> {
        pages.iter().map(|(page, _)| page.clone()).collect()
    }

    fn guarded(page: &CrawlerReviewPage) -> GuardedPage {
        GuardedPage {
            review_page_id: page.review_page_id,
            url: page.url.clone(),
        }
    }

    #[test]
    fn should_pass_a_current_live_matrix_whose_schemas_cover_every_review_page() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h2", "Table"),
        ];
        let matrix = live_matrix(
            &[schema("h1"), schema("h2")],
            &[],
            &pages,
            Duration::from_secs(60),
        );

        assert_eq!(checks(Some(&matrix), &review_pages(&pages)), Vec::new());
    }

    /// Only the server's own evaluation of the live pages vouches for an approval: neither no
    /// matrix nor the matrix schema generation stored of the pages it was given does.
    #[test]
    fn should_refuse_without_a_live_matrix() {
        let pages = [product_page("chair", "h1", "Chair")];
        let mut generated = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        generated.live_evaluation = None;

        for matrix in [None, Some(&generated)] {
            assert_eq!(
                checks(matrix, &review_pages(&pages)),
                vec![FailedApprovalCheck::LiveMatrixMissing]
            );
        }
    }

    /// A matrix of another candidate version, of another schema list or template set than
    /// approval would make live now, or of a page that left the review is outdated, whatever it
    /// shows.
    #[test]
    fn should_refuse_a_live_matrix_that_no_longer_describes_the_review() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("gone", "h1", "Gone"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        let current = review_pages(&pages);

        for refused in [
            checks_of(Some(&matrix), &current, 4, CONFIGURATION),
            checks_of(Some(&matrix), &current, 3, "another-configuration"),
            checks(Some(&matrix), &current[..1]),
        ] {
            assert!(
                matches!(
                    refused.as_slice(),
                    [FailedApprovalCheck::LiveMatrixOutdated { .. }]
                ),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn should_refuse_a_live_matrix_whose_pages_were_fetched_too_long_ago() {
        let pages = [product_page("chair", "h1", "Chair")];
        let matrix = live_matrix(
            &[schema("h1")],
            &[],
            &pages,
            MAX_AGE + Duration::from_secs(60),
        );

        let refused = checks(Some(&matrix), &review_pages(&pages));

        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::LiveMatrixExpired {
                    max_age_minutes: 30,
                    ..
                }]
            ),
            "{refused:?}"
        );
    }

    /// A page that joined after the evaluation is not covered until the matrix evaluates it, and
    /// a page no schema applies to is not covered; neither is acknowledged away.
    #[test]
    fn should_refuse_review_pages_the_matrix_did_not_evaluate_or_no_schema_covers() {
        let evaluated = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h2", "Table"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &evaluated, Duration::ZERO);
        let (joined, _) = product_page("joined", "h1", "Joined");
        let mut current = review_pages(&evaluated);
        current.push(joined.clone());

        assert_eq!(
            checks(Some(&matrix), &current),
            vec![
                FailedApprovalCheck::PagesNotEvaluated {
                    pages: vec![guarded(&joined)],
                },
                FailedApprovalCheck::PagesNotCovered {
                    pages: vec![guarded(&evaluated[1].0)],
                },
            ]
        );
    }

    /// A schema that finds the title element but no text in it leaves the product untitled; the
    /// blank title is also a plausibility finding the `auto` gate refuses.
    #[test]
    fn should_name_the_required_field_a_schema_leaves_empty_on_a_product_page() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("untitled", "h1", " "),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let refused = checks(Some(&matrix), &review_pages(&pages));

        assert_eq!(
            refused,
            vec![
                FailedApprovalCheck::PagesNotCovered {
                    pages: vec![guarded(&pages[1].0)],
                },
                FailedApprovalCheck::RequiredFieldEmpty {
                    fields: vec![EmptyRequiredField {
                        review_page_id: pages[1].0.review_page_id,
                        url: pages[1].0.url.clone(),
                        schema_index: 0,
                        field: "title",
                    }],
                },
                FailedApprovalCheck::ImplausibleExtraction {
                    pages: implausible_pages(&matrix),
                },
            ]
        );
    }

    /// The pages of a matrix whose only refusal is its implausible extractions, asserting that
    /// deterministic approval in `auto` mode refuses the same matrix, so the guard is never less
    /// strict than that gate.
    fn implausible_only(
        matrix: &SchemaMatrix,
        pages: &[CrawlerReviewPage],
    ) -> Vec<ImplausiblePage> {
        let refused = checks(Some(matrix), pages);
        assert!(
            !schema_matrix_deterministic_approval_ok(matrix),
            "the auto gate refuses it too"
        );
        match refused.as_slice() {
            [FailedApprovalCheck::ImplausibleExtraction { pages }] => {
                assert_eq!(*pages, implausible_pages(matrix));
                pages.clone()
            }
            other => panic!("expected only implausible extractions: {other:?}"),
        }
    }

    /// Two review pages show different products, so a title both extract names the shop or the
    /// category, not the product, even where the schema covers every page.
    #[test]
    fn should_refuse_a_value_the_schema_extracts_from_the_pages_of_two_products() {
        let pages = [
            product_page("chair", "h1", "Antique Shop"),
            product_page("table", "h1", "Antique Shop"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let implausible = implausible_only(&matrix, &review_pages(&pages));

        assert_eq!(implausible.len(), 2, "{implausible:?}");
        for page in &implausible {
            assert!(
                matches!(
                    page.cross_page_findings.as_slice(),
                    [finding] if finding.field == PlausibilityField::Title
                        && finding.check == CrossPageCheck::RepeatedAcrossPages
                ),
                "{page:?}"
            );
        }
    }

    /// An identity other than the one the page's listing was captured under would split the
    /// product into two listings or stop its updates.
    #[test]
    fn should_refuse_an_identity_that_differs_from_the_captured_one() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ];
        let captured = HashMap::from([(pages[0].0.url.clone(), "chair-1890".to_owned())]);
        let matrix =
            live_matrix_of_captured(&[schema("h1")], &[], &pages, Duration::ZERO, &captured);

        let implausible = implausible_only(&matrix, &review_pages(&pages));

        assert!(
            matches!(
                implausible.as_slice(),
                [page] if page.url == pages[0].0.url
                    && matches!(
                        page.identity_agreement_findings.as_slice(),
                        [finding] if finding.check
                            == IdentityAgreementCheck::DiffersFromCapturedIdentity
                    )
            ),
            "{implausible:?}"
        );
    }

    /// A per-page finding that only lowers a schema's rank, such as a price of zeros, leaves the
    /// page covered and still refuses deterministic approval, so it refuses the agent's too.
    #[test]
    fn should_refuse_a_per_page_finding_that_leaves_the_page_covered() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ]
        .map(|(page, html)| {
            (
                page,
                html.replace("</body>", "<span class=\"price\">£0.00</span></body>"),
            )
        });
        let mut priced = schema("h1");
        priced.price = Some(rule(".price", ExtractionKind::Text));
        let matrix = live_matrix(&[priced], &[], &pages, Duration::ZERO);

        let implausible = implausible_only(&matrix, &review_pages(&pages));

        assert_eq!(implausible.len(), 2, "{implausible:?}");
        for page in &implausible {
            assert!(
                page.plausibility_findings
                    .as_slice()
                    .iter()
                    .all(|finding| finding.field == PlausibilityField::Price),
                "{page:?}"
            );
        }
    }

    /// A page declared removed is resolved only where a template captures it as removed in the
    /// server's evaluation; a template that matches nothing resolves nothing.
    #[test]
    fn should_count_a_page_only_where_a_removed_page_template_captures_it() {
        let pages = [product_page("chair", "h1", "Chair"), notice_page("sold")];
        let template = |text: &str| RemovedPageSchema {
            selector: ".notice".into(),
            text: Some(text.to_owned()),
            regex: None,
        };
        let captured = live_matrix(
            &[schema("h1")],
            &[template("no longer available")],
            &pages,
            Duration::ZERO,
        );
        let unmatched = live_matrix(
            &[schema("h1")],
            &[template("sold out")],
            &pages,
            Duration::ZERO,
        );

        assert_eq!(checks(Some(&captured), &review_pages(&pages)), Vec::new());
        assert_eq!(
            checks(Some(&unmatched), &review_pages(&pages)),
            vec![FailedApprovalCheck::PagesNotCovered {
                pages: vec![guarded(&pages[1].0)],
            }]
        );
    }

    #[test]
    fn should_refuse_a_rule_that_matches_nothing_on_every_page_its_schema_applies_to() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ];
        let mut priced = schema("h1");
        priced.price = Some(rule(".price", ExtractionKind::Text));
        let matrix = live_matrix(&[priced], &[], &pages, Duration::ZERO);

        let refused = checks(Some(&matrix), &review_pages(&pages));

        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::RuleMatchesNothing { rules }]
                    if rules.len() == 1 && rules[0].field == "price" && rules[0].page_count == 2
            ),
            "{refused:?}"
        );
    }

    fn url_candidate(decision: UrlPatternDecision) -> UrlPatternReviewCandidate {
        UrlPatternReviewCandidate::new(decision, None)
    }

    fn pattern(value: &str) -> UrlPatternDecision {
        UrlPatternDecision::Pattern {
            value: value.to_owned(),
        }
    }

    fn sample() -> Vec<SampleUrl> {
        [
            ("https://antiques.example/product/chair", true),
            ("https://antiques.example/product/table", true),
            ("https://antiques.example/product/new-arrival", false),
            ("https://antiques.example/product-category/chairs", false),
            ("https://antiques.example/contact", false),
        ]
        .map(|(url, captured_product)| SampleUrl {
            url: url.to_owned(),
            captured_product,
        })
        .into()
    }

    fn uncaptured(sample: Vec<SampleUrl>) -> Vec<SampleUrl> {
        sample
            .into_iter()
            .map(|url| SampleUrl {
                captured_product: false,
                ..url
            })
            .collect()
    }

    const URL_CANDIDATE_VERSION: i64 = 2;

    /// The checks of an approval of `candidate` at [`URL_CANDIDATE_VERSION`] on `sample`, with
    /// `evaluation` stored.
    fn url_checks(
        candidate: &UrlPatternReviewCandidate,
        sample: &[SampleUrl],
        evaluation: Option<&UrlSampleEvaluation>,
    ) -> Vec<FailedApprovalCheck> {
        url_pattern_approval_checks(
            &CurrentUrlPatternReview {
                candidate,
                candidate_version: URL_CANDIDATE_VERSION,
                sample,
                evaluation,
            },
            OffsetDateTime::now_utc(),
            MAX_AGE,
        )
    }

    /// The live evaluation of `candidate` at [`URL_CANDIDATE_VERSION`] that found one schema.org
    /// `Product` on each of `products` and none on `others`, fetched `fetched_ago` before now.
    fn sample_evaluation(
        candidate: &UrlPatternReviewCandidate,
        products: &[&str],
        others: &[&str],
        fetched_ago: Duration,
    ) -> UrlSampleEvaluation {
        let pattern = candidate.decision_pattern();
        let evaluated = |url: &&str, outcome: SampleUrlOutcome| EvaluatedSampleUrl {
            url: (*url).to_owned(),
            decision_matches: pattern
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(url)),
            outcome,
        };
        UrlSampleEvaluation {
            review_id: CrawlerReviewId::new(),
            live_evaluation: LiveMatrixEvaluation {
                candidate_version: URL_CANDIDATE_VERSION,
                configuration_sha256: candidate.decision_sha256(),
                fetched_at: OffsetDateTime::now_utc() - fetched_ago,
            },
            urls: products
                .iter()
                .map(|url| evaluated(url, SampleUrlOutcome::ProductMarkup))
                .chain(
                    others
                        .iter()
                        .map(|url| evaluated(url, SampleUrlOutcome::NoProductMarkup)),
                )
                .collect(),
        }
    }

    #[test]
    fn should_pass_a_pattern_that_matches_every_captured_product_and_no_non_product_url() {
        assert_eq!(
            url_checks(&url_candidate(pattern("/product/[^/]+$")), &sample(), None),
            Vec::new()
        );
    }

    #[test]
    fn should_refuse_an_undecided_url_pattern() {
        assert_eq!(
            url_checks(
                &url_candidate(UrlPatternDecision::Undecided),
                &sample(),
                None
            ),
            vec![FailedApprovalCheck::CandidateUndecided]
        );
    }

    #[test]
    fn should_refuse_a_decision_that_misses_a_captured_product_or_matches_a_non_product_url() {
        assert_eq!(
            url_checks(&url_candidate(pattern("/product")), &sample(), None),
            vec![FailedApprovalCheck::NonProductUrlsMatched {
                urls: vec!["https://antiques.example/product-category/chairs".to_owned()],
            }]
        );
        assert_eq!(
            url_checks(&url_candidate(pattern("/product/chair")), &sample(), None),
            vec![FailedApprovalCheck::ProductUrlsNotMatched {
                urls: vec!["https://antiques.example/product/table".to_owned()],
            }]
        );
        assert_eq!(
            url_checks(
                &url_candidate(UrlPatternDecision::NoPattern),
                &sample(),
                None
            ),
            vec![FailedApprovalCheck::ProductUrlsNotMatched {
                urls: vec![
                    "https://antiques.example/product/chair".to_owned(),
                    "https://antiques.example/product/table".to_owned(),
                ],
            }]
        );
    }

    /// Without a URL known as a product page, nothing shows that a decision finds the shop's
    /// products, as on a new domain whose pages carry no product markup.
    #[test]
    fn should_refuse_a_url_pattern_while_the_sample_holds_no_known_product() {
        let candidate = url_candidate(pattern("/product/[^/]+$"));
        let without_markup = sample_evaluation(
            &candidate,
            &[],
            &["https://antiques.example/product/chair"],
            Duration::ZERO,
        );

        for evaluation in [None, Some(&without_markup)] {
            assert_eq!(
                url_checks(&candidate, &uncaptured(sample()), evaluation),
                vec![FailedApprovalCheck::NoCapturedProductUrl]
            );
        }
    }

    /// On a new domain the pages' own schema.org `Product` markup shows which sample URLs are
    /// products, and holds the decision to them as captured products do.
    #[test]
    fn should_take_product_markup_of_a_current_evaluation_as_product_evidence() {
        let fitting = url_candidate(pattern("/product/[^/]+$"));
        let too_narrow = url_candidate(pattern("/product/chair$"));
        let sample = uncaptured(sample());
        let products = [
            "https://antiques.example/product/chair",
            "https://antiques.example/product/table",
        ];

        assert_eq!(
            url_checks(
                &fitting,
                &sample,
                Some(&sample_evaluation(
                    &fitting,
                    &products,
                    &["https://antiques.example/product/new-arrival"],
                    Duration::from_secs(60)
                ))
            ),
            Vec::new()
        );
        assert_eq!(
            url_checks(
                &too_narrow,
                &sample,
                Some(&sample_evaluation(
                    &too_narrow,
                    &products,
                    &[],
                    Duration::ZERO
                ))
            ),
            vec![FailedApprovalCheck::ProductUrlsNotMatched {
                urls: vec!["https://antiques.example/product/table".to_owned()],
            }]
        );
    }

    /// An evaluation made for another decision, at another candidate version or on URLs outside
    /// the sample vouches for nothing, and an old one has expired, whatever it found.
    #[test]
    fn should_refuse_a_sample_evaluation_that_no_longer_describes_the_review() {
        let fitting = url_candidate(pattern("/product/[^/]+$"));
        let sample = uncaptured(sample());
        let products = ["https://antiques.example/product/chair"];
        let of_another_decision = sample_evaluation(
            &url_candidate(pattern("/product/")),
            &products,
            &[],
            Duration::ZERO,
        );
        let mut of_another_version = sample_evaluation(&fitting, &products, &[], Duration::ZERO);
        of_another_version.live_evaluation.candidate_version = URL_CANDIDATE_VERSION - 1;
        let of_another_url = sample_evaluation(
            &fitting,
            &["https://antiques.example/product/elsewhere"],
            &[],
            Duration::ZERO,
        );

        for outdated in [of_another_decision, of_another_version, of_another_url] {
            let refused = url_checks(&fitting, &sample, Some(&outdated));
            assert!(
                matches!(
                    refused.as_slice(),
                    [FailedApprovalCheck::LiveMatrixOutdated { .. }]
                ),
                "{refused:?}"
            );
        }
        let expired =
            sample_evaluation(&fitting, &products, &[], MAX_AGE + Duration::from_secs(60));
        let refused = url_checks(&fitting, &sample, Some(&expired));
        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::LiveMatrixExpired {
                    max_age_minutes: 30,
                    ..
                }]
            ),
            "{refused:?}"
        );
    }

    /// The refusal names each failed check by a machine-readable code next to what it lists.
    #[test]
    fn should_name_each_failed_check_by_its_code() {
        let page = GuardedPage {
            review_page_id: CrawlerReviewPageId::new(),
            url: "https://antiques.example/products/chair".to_owned(),
        };

        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::PagesNotCovered {
                pages: vec![page.clone()],
            })
            .ok(),
            Some(serde_json::json!({
                "check": "PAGES_NOT_COVERED",
                "pages": [{ "review_page_id": page.review_page_id, "url": page.url }],
            }))
        );
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::LiveMatrixMissing).ok(),
            Some(serde_json::json!({ "check": "LIVE_MATRIX_MISSING" }))
        );
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::ApproverIsAuthor).ok(),
            Some(serde_json::json!({ "check": "APPROVER_IS_AUTHOR" }))
        );
    }
}
