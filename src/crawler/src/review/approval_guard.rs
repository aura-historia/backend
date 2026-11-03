//! The deterministic checks an approval by a review agent must pass in `agent` authoring mode. A
//! check reads only what the server itself evaluated and stored, a product-schema review's live
//! matrix or a URL-pattern candidate's sample labels, never a matrix or a verdict a client sends
//! with the approval; the contract is in `docs/crawler/extraction-and-review.md` (approval guard).

use crate::CrawlerReviewPageId;
use crate::network::policy::{canonical_crawler_domain, url_matches_configured_domain};
use crate::review::model::{
    CrawlerReviewPage, LabelledSampleUrl, LiveMatrixEvaluation, SampleUrlLabel, SchemaMatrix,
    SchemaPageReference, UrlPatternDecision, UrlPatternReviewCandidate,
};
use crate::review::page_coverage::{PageCoverage, page_coverages};
use crate::review::schema_evaluation::{
    ImplausiblePage, OptionalRuleTotalMiss, implausible_pages, optional_rules_missing_on_every_page,
};
use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField};
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;
use serde::Serialize;
use std::fmt;
use std::num::NonZeroU32;
use std::time::Duration;
use time::OffsetDateTime;
use url::Url;

/// One check a guarded agent approval failed, as the refusal names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "check", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailedApprovalCheck {
    /// The approving agent itself changed the candidate version it approves, or the pages it is
    /// evaluated on: no agent approves its own work.
    ApproverIsAuthor,
    /// The review stores no schema matrix the server evaluated on its live pages.
    LiveMatrixMissing,
    /// The stored live matrix evaluated another candidate version, or another configuration than
    /// approval would make live now: another schema list or other removed-page templates; or a
    /// page that left the review.
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
    /// Candidate extractions that fail a per-page, cross-page or identity agreement plausibility
    /// check, which deterministic approval in `auto` mode refuses too, on any page.
    ImplausibleExtraction { pages: Vec<ImplausiblePage> },
    /// The URL-pattern candidate decides nothing yet.
    CandidateUndecided,
    /// The candidate's sample labels on the review's domain mark fewer than `minimum` URLs as
    /// products or fewer than `minimum` as no products, so they cannot show that the decision
    /// takes the domain's product pages and leaves the rest.
    TooFewLabels {
        product: usize,
        not_product: usize,
        minimum: u32,
    },
    /// URLs labelled `PRODUCT` that the decision does not classify as product pages.
    ProductLabelNotMatched { urls: Vec<String> },
    /// URLs labelled `NOT_PRODUCT` that the decision classifies as product pages under a spelling
    /// the crawler may store them under.
    NotProductLabelMatched { urls: Vec<String> },
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
            Self::TooFewLabels {
                product,
                not_product,
                minimum,
            } => write!(
                formatter,
                "the sample labels mark {product} URL(s) of the review's domain PRODUCT and {not_product} NOT_PRODUCT; an agent approval needs at least {minimum} of each, which the author agent or a human labels"
            ),
            Self::ProductLabelNotMatched { urls } => write!(
                formatter,
                "the decision does not take {} URL(s) labelled PRODUCT",
                urls.len()
            ),
            Self::NotProductLabelMatched { urls } => write!(
                formatter,
                "the decision takes {} URL(s) labelled NOT_PRODUCT",
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

/// The checks an agent approval of the URL-pattern `candidate` fails, none when it may go ahead:
/// the candidate is decided, and its sample labels, which the author agent or a human set and the
/// approval rests on alone, are consistent with the decision. Only labels of URLs on the review's
/// `domain`, the one domain the decision applies to, count: a label on another domain of the
/// ListingSource says nothing about it. They mark at least `min_labelled_urls` URLs as products
/// and as many as no products, and the decision classifies every `PRODUCT` URL as a product page
/// and no `NOT_PRODUCT` URL. Labels belong to the candidate, so the approved candidate version
/// names the labels it is judged on, and they do not expire. No URL is classified by keywords or
/// by its page's markup here: Claude decides what a product page is, and the crawler only checks
/// that the decision follows its labels.
pub(crate) fn url_pattern_approval_checks(
    candidate: &UrlPatternReviewCandidate,
    domain: &str,
    min_labelled_urls: NonZeroU32,
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
    let on_domain: Vec<&LabelledSampleUrl> = candidate
        .sample_labels
        .iter()
        .flat_map(|labels| &labels.labels)
        .filter(|url| is_on_domain(&url.url, domain))
        .collect();
    let labelled = |label: SampleUrlLabel| {
        on_domain
            .iter()
            .copied()
            .filter(move |url| url.label == label)
    };
    let mut failed = Vec::new();
    let minimum = min_labelled_urls.get();
    let (product, not_product) = (
        labelled(SampleUrlLabel::Product).count(),
        labelled(SampleUrlLabel::NotProduct).count(),
    );
    if product.min(not_product) < usize::try_from(minimum).unwrap_or(usize::MAX) {
        failed.push(FailedApprovalCheck::TooFewLabels {
            product,
            not_product,
            minimum,
        });
    }
    let missed: Vec<String> = labelled(SampleUrlLabel::Product)
        .filter(|url| !decision_takes(pattern.as_ref(), &url.url))
        .map(|url| url.url.clone())
        .collect();
    if !missed.is_empty() {
        failed.push(FailedApprovalCheck::ProductLabelNotMatched { urls: missed });
    }
    let taken: Vec<String> = labelled(SampleUrlLabel::NotProduct)
        .filter(|url| decision_takes_any_spelling(pattern.as_ref(), &url.url))
        .map(|url| url.url.clone())
        .collect();
    if !taken.is_empty() {
        failed.push(FailedApprovalCheck::NotProductLabelMatched { urls: taken });
    }
    failed
}

/// Whether the labelled `url` is on `domain`, under the crawler's host identity, which treats
/// `www.` and the bare host as one domain.
fn is_on_domain(url: &str, domain: &str) -> bool {
    Url::parse(url).is_ok_and(|url| url_matches_configured_domain(&url, domain))
}

/// Whether the decision's `pattern` takes the labelled `url` as a product page, matched in the
/// crawler's URL spelling, with or without its trailing slash, as the spider matches it;
/// `NO_PATTERN` takes none.
fn decision_takes(pattern: Option<&Regex>, url: &str) -> bool {
    pattern.is_some_and(|pattern| {
        Url::parse(url).is_ok_and(|url| CrawledUrl::new(url).matches_pattern(pattern))
    })
}

/// Whether the decision's `pattern` takes the page the labelled `url` names under any spelling the
/// crawler may store it under: the crawler's spelling of the URL on http or https, with or without
/// `www.`, which the crawler treats as one domain, each with or without its trailing slash and in
/// its [legacy spellings](CrawledUrl::legacy_spellings), under which an earlier row stays until the
/// spider rediscovers the page and which approval classifies as it is stored. A decision that
/// takes only one of them would otherwise pass a `NOT_PRODUCT` label in another and still take the
/// page as the crawler stored it.
fn decision_takes_any_spelling(pattern: Option<&Regex>, url: &str) -> bool {
    let Some(pattern) = pattern else {
        return false;
    };
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    let url = CrawledUrl::new(url).as_url().clone();
    let Some(domain) = url.host_str().map(canonical_crawler_domain) else {
        return CrawledUrl::new(url).matches_pattern(pattern);
    };
    let www = format!("www.{domain}");
    ["https", "http"]
        .into_iter()
        .flat_map(|scheme| [(scheme, domain.as_str()), (scheme, www.as_str())])
        .filter_map(|(scheme, host)| {
            let mut spelling = url.clone();
            (spelling.set_scheme(scheme).is_ok() && spelling.set_host(Some(host)).is_ok())
                .then_some(spelling)
        })
        .chain(std::iter::once(url.clone()))
        .flat_map(|spelling| {
            let legacy = CrawledUrl::new(spelling.clone()).legacy_spellings();
            std::iter::once(spelling).chain(legacy)
        })
        .any(|spelling| CrawledUrl::new(spelling).matches_pattern(pattern))
}

/// The refusal of a live matrix whose pages were fetched more than `max_age` before `now`.
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
    use crate::review::model::{PAGE_ROLE_PRIMARY, SampleLabels};
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
            &[],
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

    fn pattern(value: &str) -> UrlPatternDecision {
        UrlPatternDecision::Pattern {
            value: value.to_owned(),
        }
    }

    const MIN_LABELLED_URLS: NonZeroU32 = NonZeroU32::new(3).unwrap();
    /// The domain the URL-pattern review decides, on which [`PRODUCTS`] and [`NOT_PRODUCTS`] are.
    const REVIEW_DOMAIN: &str = "antique-shop.example";
    const PRODUCTS: [&str; 3] = [
        "https://antique-shop.example/item/oak-chair",
        "https://antique-shop.example/item/pine-table",
        "https://antique-shop.example/item/brass-lamp/",
    ];
    const NOT_PRODUCTS: [&str; 3] = [
        "https://antique-shop.example/item-category/chairs",
        "https://antique-shop.example/shop/",
        "https://antique-shop.example/impressum",
    ];

    /// `decision` with `products` labelled `PRODUCT` and `not_products` labelled `NOT_PRODUCT`,
    /// the labels set a year before now: labels do not expire.
    fn labelled(
        decision: UrlPatternDecision,
        products: &[&str],
        not_products: &[&str],
    ) -> UrlPatternReviewCandidate {
        let label = |url: &&str, label: SampleUrlLabel| LabelledSampleUrl {
            url: (*url).to_owned(),
            label,
            reason: "seen on the page".to_owned(),
        };
        UrlPatternReviewCandidate {
            sample_labels: Some(SampleLabels {
                labelled_at: OffsetDateTime::now_utc() - Duration::from_secs(365 * 24 * 60 * 60),
                labels: products
                    .iter()
                    .map(|url| label(url, SampleUrlLabel::Product))
                    .chain(
                        not_products
                            .iter()
                            .map(|url| label(url, SampleUrlLabel::NotProduct)),
                    )
                    .collect(),
            }),
            ..UrlPatternReviewCandidate::new(decision, None)
        }
    }

    fn url_checks(candidate: &UrlPatternReviewCandidate) -> Vec<FailedApprovalCheck> {
        url_pattern_approval_checks(candidate, REVIEW_DOMAIN, MIN_LABELLED_URLS)
    }

    /// A decision that takes every URL labelled `PRODUCT`, in the crawler's spelling of it, and no
    /// URL labelled `NOT_PRODUCT` passes on a shop whose domain the keywords read as a category,
    /// however long ago the labels were set.
    #[test]
    fn should_pass_a_decision_consistent_with_the_labels() {
        let candidate = labelled(pattern("/item/[^/]+$"), &PRODUCTS, &NOT_PRODUCTS);

        assert_eq!(url_checks(&candidate), Vec::new());
    }

    #[test]
    fn should_refuse_an_undecided_url_pattern() {
        let candidate = labelled(UrlPatternDecision::Undecided, &PRODUCTS, &NOT_PRODUCTS);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::CandidateUndecided]
        );
    }

    /// Fewer than the minimum of either label shows too little, whatever the decision; without
    /// labels nothing shows anything.
    #[test]
    fn should_refuse_too_few_labels_of_either_kind() {
        let too_few = |candidate: UrlPatternReviewCandidate, product, not_product| {
            assert_eq!(
                url_checks(&candidate),
                vec![FailedApprovalCheck::TooFewLabels {
                    product,
                    not_product,
                    minimum: 3,
                }]
            );
        };

        too_few(
            labelled(pattern("/item/[^/]+$"), &PRODUCTS[..2], &NOT_PRODUCTS),
            2,
            3,
        );
        too_few(labelled(pattern("/item/[^/]+$"), &PRODUCTS, &[]), 3, 0);
        too_few(
            UrlPatternReviewCandidate::new(pattern("/item/[^/]+$"), None),
            0,
            0,
        );
    }

    /// The decision applies to the review's domain alone, so labels on another domain of the
    /// ListingSource say nothing about it: they do not count towards the minimum, however many
    /// there are and however well the decision follows them.
    #[test]
    fn should_count_only_labels_on_the_review_domain() {
        let elsewhere =
            |urls: [&str; 3]| urls.map(|url| url.replace(REVIEW_DOMAIN, "antique-shop.de"));
        let (other_products, other_not_products) = (elsewhere(PRODUCTS), elsewhere(NOT_PRODUCTS));
        let products: Vec<&str> = std::iter::once(PRODUCTS[0])
            .chain(other_products.iter().map(String::as_str))
            .collect();
        let not_products: Vec<&str> = std::iter::once(NOT_PRODUCTS[0])
            .chain(other_not_products.iter().map(String::as_str))
            .collect();
        let candidate = labelled(pattern("/item/[^/]+$"), &products, &not_products);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::TooFewLabels {
                product: 1,
                not_product: 1,
                minimum: 3,
            }]
        );
    }

    #[test]
    fn should_refuse_a_decision_that_misses_a_url_labelled_product() {
        let candidate = labelled(pattern("/item/(oak|pine)-"), &PRODUCTS, &NOT_PRODUCTS);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::ProductLabelNotMatched {
                urls: vec![PRODUCTS[2].to_owned()],
            }]
        );
    }

    #[test]
    fn should_refuse_a_decision_that_takes_a_url_labelled_not_product() {
        let candidate = labelled(pattern("/item"), &PRODUCTS, &NOT_PRODUCTS);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::NotProductLabelMatched {
                urls: vec![NOT_PRODUCTS[0].to_owned()],
            }]
        );
    }

    /// The crawler treats `www.` and the bare host, http and https as one page and stores
    /// whichever spelling it found, so a decision anchored on a spelling the label does not use
    /// still takes the page labelled `NOT_PRODUCT`; one that takes no spelling of it passes.
    #[test]
    fn should_refuse_a_decision_that_takes_a_url_labelled_not_product_in_another_spelling() {
        let www_products = PRODUCTS.map(|url| url.replace("https://", "https://www."));
        let www_products: Vec<&str> = www_products.iter().map(String::as_str).collect();
        let on_www = |value: &str| labelled(pattern(value), &www_products, &NOT_PRODUCTS);

        assert_eq!(
            url_checks(&on_www(r"^https://www\.antique-shop\.example/")),
            vec![FailedApprovalCheck::NotProductLabelMatched {
                urls: NOT_PRODUCTS.map(str::to_owned).to_vec(),
            }]
        );
        assert_eq!(
            url_checks(&on_www(r"^http://antique-shop\.example/")),
            vec![
                FailedApprovalCheck::ProductLabelNotMatched {
                    urls: www_products.iter().map(|url| (*url).to_owned()).collect(),
                },
                FailedApprovalCheck::NotProductLabelMatched {
                    urls: NOT_PRODUCTS.map(str::to_owned).to_vec(),
                },
            ]
        );
        assert_eq!(
            url_checks(&on_www(r"^https://www\.antique-shop\.example/item/[^/]+$")),
            Vec::new()
        );
    }

    /// The spider stores a URL with the trailing slash it fetched it with, and a pattern takes a
    /// page with or without its slash: one written for the slashed spelling takes `PRODUCT`
    /// labels without it, and a `NOT_PRODUCT` label without it as well.
    #[test]
    fn should_match_labels_with_or_without_their_trailing_slash() {
        let gift_card = "https://antique-shop.example/item/gift-card";
        let not_products = [gift_card, NOT_PRODUCTS[1], NOT_PRODUCTS[2]];
        let candidate = labelled(pattern(r"/item/[^/]+/$"), &PRODUCTS, &not_products);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::NotProductLabelMatched {
                urls: vec![gift_card.to_owned()],
            }]
        );
    }

    /// Rows stored while the crawler sorted query parameters keep that spelling until the spider
    /// rediscovers them, and approval classifies them as stored: a decision that takes the
    /// labelled `NOT_PRODUCT` page only in its legacy spelling still takes that row.
    #[test]
    fn should_refuse_a_decision_that_takes_a_url_labelled_not_product_in_its_legacy_spelling() {
        let products = [1, 2, 3].map(|id| {
            format!("https://antique-shop.example/index.php?controller=product&id_product={id}")
        });
        let products: Vec<&str> = products.iter().map(String::as_str).collect();
        let category = "https://antique-shop.example/index.php?id_category=3&controller=category";
        let not_products = [category, NOT_PRODUCTS[1], NOT_PRODUCTS[2]];
        let deciding = |value: &str| labelled(pattern(value), &products, &not_products);

        assert_eq!(
            url_checks(&deciding(r"/index\.php\?controller=")),
            vec![FailedApprovalCheck::NotProductLabelMatched {
                urls: vec![category.to_owned()],
            }]
        );
        assert_eq!(
            url_checks(&deciding(r"/index\.php\?controller=product&")),
            Vec::new()
        );
    }

    /// `NO_PATTERN` takes no URL, so labelled products always refuse it: a human decides it.
    #[test]
    fn should_refuse_no_pattern_against_urls_labelled_product() {
        let candidate = labelled(UrlPatternDecision::NoPattern, &PRODUCTS, &NOT_PRODUCTS);

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::ProductLabelNotMatched {
                urls: PRODUCTS.map(str::to_owned).to_vec(),
            }]
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
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::TooFewLabels {
                product: 1,
                not_product: 0,
                minimum: 3,
            })
            .ok(),
            Some(serde_json::json!({
                "check": "TOO_FEW_LABELS",
                "product": 1,
                "not_product": 0,
                "minimum": 3,
            }))
        );
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::NotProductLabelMatched {
                urls: vec!["https://antiques.example/shop".to_owned()],
            })
            .ok(),
            Some(serde_json::json!({
                "check": "NOT_PRODUCT_LABEL_MATCHED",
                "urls": ["https://antiques.example/shop"],
            }))
        );
    }
}
