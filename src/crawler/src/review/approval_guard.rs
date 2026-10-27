//! The deterministic checks an approval by the review agent must pass while its approvals are
//! `guarded`. A check reads only what the server itself evaluated and stored, never a matrix or
//! a verdict a client sends; the contract is in `docs/crawler/extraction-and-review.md` (review
//! rail).

use crate::CrawlerReviewPageId;
use crate::review::model::{
    CrawlerReviewPage, LiveMatrixEvaluation, SchemaMatrix, SchemaPageReference, UrlPatternDecision,
    UrlPatternReviewCandidate,
};
use crate::review::page_coverage::{PageCoverage, page_coverages};
use crate::review::schema_evaluation::{
    OptionalRuleTotalMiss, optional_rules_missing_on_every_page,
};
use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField};
use crate::spider::classification::url_metadata::UrlClass;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;
use serde::Serialize;
use std::fmt;
use std::time::Duration;
use time::OffsetDateTime;
use url::Url;

/// One check a guarded agent approval failed, as the refusal names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "check", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailedApprovalCheck {
    /// The review stores no schema matrix the server evaluated on its live pages.
    LiveMatrixMissing,
    /// The stored live matrix evaluated another candidate version, another schema list or other
    /// removed-page templates than approval would make live now, or a page that left the review.
    LiveMatrixOutdated { reason: String },
    /// The stored live matrix's pages were fetched longer ago than allowed.
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
    /// The URL-pattern candidate decides nothing yet.
    CandidateUndecided,
    /// The review's URL sample holds no URL the crawler captured a product from, so nothing
    /// shows that the pattern finds products.
    NoCapturedProductUrl,
    /// Sample URLs the crawler captured a product from that the pattern does not match.
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
            Self::LiveMatrixMissing => write!(
                formatter,
                "no schema matrix evaluated on the live review pages is stored; evaluate the matrix"
            ),
            Self::LiveMatrixOutdated { reason } => write!(
                formatter,
                "the stored live matrix is outdated ({reason}); evaluate the matrix again"
            ),
            Self::LiveMatrixExpired {
                max_age_minutes, ..
            } => write!(
                formatter,
                "the stored live matrix's pages were fetched more than {max_age_minutes} minutes ago; evaluate the matrix again"
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
            Self::CandidateUndecided => write!(formatter, "the URL-pattern candidate is undecided"),
            Self::NoCapturedProductUrl => write!(
                formatter,
                "the review's URL sample holds no URL the crawler captured a product from"
            ),
            Self::ProductUrlsNotMatched { urls } => write!(
                formatter,
                "the pattern misses {} sample URL(s) the crawler captured a product from",
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
/// product page; and no rule matches nothing on every page its schema applies to.
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
    if now - evaluation.fetched_at > max_age {
        failed.push(FailedApprovalCheck::LiveMatrixExpired {
            fetched_at: evaluation.fetched_at,
            max_age_minutes: max_age.as_secs() / 60,
        });
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

/// A URL of a URL-pattern review's sample, with whether the crawler captured a product from it.
pub(crate) struct SampleUrl {
    pub url: String,
    pub captured_product: bool,
}

/// The checks an agent approval of a URL-pattern review fails, none when it may go ahead: the
/// candidate is decided, the review's URL sample holds a URL the crawler captured a product
/// from, and the decision classifies as products every such URL and none of the sample URLs the
/// crawler's own URL classification puts in the category, imprint or info class.
pub(crate) fn url_pattern_approval_checks(
    candidate: &UrlPatternReviewCandidate,
    sample: &[SampleUrl],
) -> Vec<FailedApprovalCheck> {
    let pattern = match &candidate.decision {
        UrlPatternDecision::Undecided => return vec![FailedApprovalCheck::CandidateUndecided],
        UrlPatternDecision::NoPattern => None,
        UrlPatternDecision::Pattern { value } => match Regex::new(value) {
            Ok(pattern) => Some(pattern),
            // Approval refuses an invalid pattern on its own.
            Err(_) => return Vec::new(),
        },
    };
    let classified_as_product = |url: &str| {
        crawled_url(url)
            .is_some_and(|url| url.classify(pattern.as_ref()) == UrlClass::ProductListing)
    };
    let mut failed = Vec::new();
    let products: Vec<&SampleUrl> = sample.iter().filter(|url| url.captured_product).collect();
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
    let matched_non_products: Vec<String> = sample
        .iter()
        .filter(|url| !url.captured_product && is_non_product_url(&url.url))
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

fn crawled_url(url: &str) -> Option<CrawledUrl> {
    Url::parse(url).ok().map(CrawledUrl::new)
}

/// A URL the crawler classifies as a category, imprint or info page without a product pattern.
fn is_non_product_url(url: &str) -> bool {
    crawled_url(url).is_some_and(|url| {
        matches!(
            url.classify(None),
            UrlClass::Category | UrlClass::Imprint | UrlClass::Info
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::PAGE_ROLE_PRIMARY;
    use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
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
        let mut matrix = evaluate_schema_matrix_for_live_review_pages(
            CrawlerReviewId::new(),
            schemas,
            templates,
            pages,
            &HashMap::new(),
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

    /// A schema that finds the title element but no text in it leaves the product untitled.
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
            ]
        );
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

    #[test]
    fn should_pass_a_pattern_that_matches_every_captured_product_and_no_non_product_url() {
        assert_eq!(
            url_pattern_approval_checks(&url_candidate(pattern("/product/[^/]+$")), &sample()),
            Vec::new()
        );
    }

    #[test]
    fn should_refuse_an_undecided_url_pattern() {
        assert_eq!(
            url_pattern_approval_checks(&url_candidate(UrlPatternDecision::Undecided), &sample()),
            vec![FailedApprovalCheck::CandidateUndecided]
        );
    }

    #[test]
    fn should_refuse_a_decision_that_misses_a_captured_product_or_matches_a_non_product_url() {
        assert_eq!(
            url_pattern_approval_checks(&url_candidate(pattern("/product")), &sample()),
            vec![FailedApprovalCheck::NonProductUrlsMatched {
                urls: vec!["https://antiques.example/product-category/chairs".to_owned()],
            }]
        );
        assert_eq!(
            url_pattern_approval_checks(&url_candidate(pattern("/product/chair")), &sample()),
            vec![FailedApprovalCheck::ProductUrlsNotMatched {
                urls: vec!["https://antiques.example/product/table".to_owned()],
            }]
        );
        assert_eq!(
            url_pattern_approval_checks(&url_candidate(UrlPatternDecision::NoPattern), &sample()),
            vec![FailedApprovalCheck::ProductUrlsNotMatched {
                urls: vec![
                    "https://antiques.example/product/chair".to_owned(),
                    "https://antiques.example/product/table".to_owned(),
                ],
            }]
        );
    }

    /// Without a URL the crawler captured a product from, nothing shows that a decision finds
    /// the shop's products, as for a domain no pattern was approved for yet.
    #[test]
    fn should_refuse_a_url_pattern_while_the_sample_holds_no_captured_product() {
        let uncaptured: Vec<SampleUrl> = sample()
            .into_iter()
            .map(|url| SampleUrl {
                captured_product: false,
                ..url
            })
            .collect();

        assert_eq!(
            url_pattern_approval_checks(&url_candidate(pattern("/product/[^/]+$")), &uncaptured),
            vec![FailedApprovalCheck::NoCapturedProductUrl]
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
    }
}
