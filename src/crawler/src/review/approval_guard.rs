//! The deterministic checks an approval by a review agent must pass in `agent` authoring mode. A
//! check reads only what the server itself evaluated and stored, a product-schema review's live
//! matrix or a URL-pattern review's stored sample of crawled URLs, never a matrix or a verdict a client sends
//! with the approval; the contract is in `docs/crawler/extraction-and-review.md` (approval guard).

use crate::CrawlerReviewPageId;
use crate::network::policy::url_on_crawl_host;
use crate::review::model::{
    CrawlerReviewPage, LiveMatrixEvaluation, SchemaMatrix, SchemaPageReference, UrlPatternDecision,
    UrlPatternReviewCandidate,
};
use crate::review::page_content_checks::{
    MissingContentRule, SeenState, missing_content_rules, pages_without_signals,
    shows_sold_or_unavailable_page, states_seen,
};
use crate::review::page_coverage::{PageCoverage, page_coverages};
use crate::review::schema_evaluation::{
    ImplausiblePage, OptionalRuleTotalMiss, implausible_pages, optional_rules_missing_on_every_page,
};
use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField};
use crate::spider::utils::url::CrawledUrl;
use serde::Serialize;
use std::fmt;
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
    /// Review pages that show a price or a description block while no schema applying to them
    /// has a `price` or a `description` rule.
    ContentRuleMissing { rules: Vec<MissingContentRule> },
    /// No review page shows a sold or unavailable product, and the review records no search for
    /// one that found none. `states_seen` lists what the candidate schemas extract as `state` on
    /// the pages they apply to, and what the normalization resolves each to.
    NoSoldPageTested { states_seen: Vec<SeenState> },
    /// The URL-pattern candidate decides nothing yet.
    CandidateUndecided,
    /// The candidate's pattern takes none of the review's sample URLs: it describes no product
    /// URL the crawl found, such as one written for another URL shape or another host, and an
    /// empty sample shows nothing.
    PatternMatchesNoSampleUrl { sample_total: usize },
    /// The candidate's pattern takes every URL of a sample of at least two: it tells no product
    /// page from the shop's other pages, such as one that takes the whole host.
    PatternMatchesEverySampleUrl { sample_total: usize },
    /// A `NO_PATTERN` decision, which an agent never approves: nothing in a review shows that a
    /// shop has no product URLs, so a human decides it.
    NoPatternNeedsHuman,
}

/// The states of a `NO_SOLD_PAGE_TESTED` refusal, each as `'state' → CODE`, `→ CODE mentioned` or
/// `→ unresolved`.
/// The checks that block `page`, as `title REPEATED_ACROSS_PAGES on <url>`.
fn describe_blocking_checks(page: &ImplausiblePage) -> String {
    let checks = page
        .plausibility_findings
        .as_slice()
        .iter()
        .map(|finding| (&finding.field, check_code(&finding.check)))
        .chain(
            page.cross_page_findings
                .as_slice()
                .iter()
                .map(|finding| (&finding.field, check_code(&finding.check))),
        )
        .chain(
            page.identity_agreement_findings
                .as_slice()
                .iter()
                .map(|finding| (&finding.field, check_code(&finding.check))),
        )
        .map(|(field, code)| format!("{} {code}", field.as_str()))
        .collect::<Vec<_>>();
    format!("{} on {}", checks.join(", "), page.url)
}

/// The check's persisted `SCREAMING_SNAKE_CASE` code, as the matrix API names it.
fn check_code(check: &impl serde::Serialize) -> String {
    match serde_json::to_value(check) {
        Ok(serde_json::Value::String(code)) => code,
        _ => "UNKNOWN".to_owned(),
    }
}

fn describe_states(states: &[SeenState]) -> String {
    if states.is_empty() {
        return "none, no candidate schema applies to a review page".to_owned();
    }
    states
        .iter()
        .map(|seen| {
            format!(
                "'{}' → {} on {}",
                seen.state,
                match (seen.resolves_to, seen.mentions) {
                    (Some(code), _) => code.to_owned(),
                    (None, Some(code)) => format!("{code} mentioned"),
                    (None, None) => "unresolved".to_owned(),
                },
                seen.url
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
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
                "{} extraction(s) fail a plausibility check deterministic approval refuses: {}",
                pages.len(),
                pages
                    .iter()
                    .map(describe_blocking_checks)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::ContentRuleMissing { rules } => write!(
                formatter,
                "the review pages show content no rule captures: {}; add the rule, or the page shows none",
                rules
                    .iter()
                    .map(|rule| format!("{} on {} page(s)", rule.field, rule.pages.len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::NoSoldPageTested { states_seen } => write!(
                formatter,
                "no review page's state resolves to sold or unavailable; states seen: {}; add a sold page with add_review_page and refresh the matrix, or record the pages you checked with record_no_sold_page when the shop has none",
                describe_states(states_seen)
            ),
            Self::CandidateUndecided => write!(formatter, "the URL-pattern candidate is undecided"),
            Self::PatternMatchesNoSampleUrl { sample_total } => write!(
                formatter,
                "the pattern takes none of the sample's {sample_total} URL(s): it describes no product URL the crawl found; write it for the product URLs the sample shows"
            ),
            Self::PatternMatchesEverySampleUrl { sample_total } => write!(
                formatter,
                "the pattern takes every one of the sample's {sample_total} URLs: it tells no product page from the shop's other pages; write it for the product URLs alone"
            ),
            Self::NoPatternNeedsHuman => write!(
                formatter,
                "an agent does not approve NO_PATTERN; a human decides it"
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
    /// Whether the review's trail records that the author looked for a sold or unavailable page
    /// of the ListingSource and found none.
    pub no_sold_page_recorded: bool,
}

/// The checks an agent approval of a product-schema review fails, none when it may go ahead: the
/// review's stored `matrix` is a live evaluation of exactly what approval would make live, of
/// exactly the review's current product pages (a page marked as no product page is no page of the
/// guard: the matrix leaves it out and no check reads it), fetched within `max_age` of `now`; every review page is
/// covered by a schema or captured as removed; no schema leaves a required field empty on a
/// product page; no rule matches nothing on every page its schema applies to; no extraction fails
/// a plausibility check that deterministic approval refuses, so the guard is never less strict
/// than the `auto` gate; a review page shows a sold or unavailable product, or the review records
/// that its author found none; and no page shows a price or a description no rule captures.
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
    if !review.no_sold_page_recorded && !shows_sold_or_unavailable_page(matrix, &coverages) {
        failed.push(FailedApprovalCheck::NoSoldPageTested {
            states_seen: states_seen(matrix, &coverages),
        });
    }
    let content_rules = missing_content_rules(matrix, &coverages);
    if !content_rules.is_empty() {
        failed.push(FailedApprovalCheck::ContentRuleMissing {
            rules: content_rules,
        });
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
    if pages_without_signals(matrix) {
        return Some(
            "it predates the page signals the guard reads; evaluate the matrix again".to_owned(),
        );
    }
    let evaluates_a_review_page = |reference: &SchemaPageReference| match reference {
        SchemaPageReference::Persisted { review_page_id } => review
            .pages
            .iter()
            .any(|page| page.review_page_id == *review_page_id && !page.is_marked_not_product()),
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

/// The checks an agent approval of the URL-pattern `candidate` fails, none when it may go ahead.
/// Induction is label-blind, so the checks are the local ones and read no label and no page: the
/// candidate is decided, its pattern compiles (decoding the candidate guarantees it), and it takes
/// at least one URL of the review's `sample` of crawled URLs and, on a sample of at least two, not
/// every one. Only sample URLs on `domain`, the crawl host of the review's ListingSource and the
/// one host the decision applies to, count. They show the pattern is a usable rule, not that it is right: no URL is classified
/// by keywords or by its page's markup here, Claude decides what a product page is. A
/// `NO_PATTERN` decision is a human's to approve.
pub(crate) fn url_pattern_approval_checks(
    candidate: &UrlPatternReviewCandidate,
    domain: &str,
    sample: &[String],
) -> Vec<FailedApprovalCheck> {
    let pattern = match &candidate.decision {
        UrlPatternDecision::Undecided => return vec![FailedApprovalCheck::CandidateUndecided],
        decision => match decision.candidate_pattern() {
            Ok(Some(pattern)) => pattern,
            Ok(None) => return vec![FailedApprovalCheck::NoPatternNeedsHuman],
            // Approval refuses an invalid pattern on its own.
            Err(_) => return Vec::new(),
        },
    };
    let on_domain: Vec<Url> = sample
        .iter()
        .filter_map(|url| Url::parse(url).ok())
        .filter(|url| url_on_crawl_host(url, domain))
        .collect();
    let sample_total = on_domain.len();
    let matched = on_domain
        .into_iter()
        .filter(|url| CrawledUrl::new(url.clone()).matches_pattern(&pattern))
        .count();
    if matched == 0 {
        vec![FailedApprovalCheck::PatternMatchesNoSampleUrl { sample_total }]
    } else if sample_total >= 2 && matched == sample_total {
        vec![FailedApprovalCheck::PatternMatchesEverySampleUrl { sample_total }]
    } else {
        Vec::new()
    }
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
    use crate::review::model::PAGE_ROLE_PRIMARY;
    use crate::review::schema_evaluation::{
        evaluate_schema_matrix_for_live_review_pages, schema_matrix_deterministic_approval_ok,
    };
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::scraper::plausibility::{CrossPageCheck, IdentityAgreementCheck};
    use crate::spider::classification::product_url_pattern::UrlPatternMatching;
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
                not_product_reason: None,
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

    /// The checks of a review whose trail records that its author found no sold page, so the
    /// product pages that show "Available" test no sold page without refusing it.
    fn checks_of(
        matrix: Option<&SchemaMatrix>,
        pages: &[CrawlerReviewPage],
        candidate_version: i64,
        configuration_sha256: &str,
    ) -> Vec<FailedApprovalCheck> {
        checks_recorded(matrix, pages, candidate_version, configuration_sha256, true)
    }

    fn checks_recorded(
        matrix: Option<&SchemaMatrix>,
        pages: &[CrawlerReviewPage],
        candidate_version: i64,
        configuration_sha256: &str,
        no_sold_page_recorded: bool,
    ) -> Vec<FailedApprovalCheck> {
        product_schema_approval_checks(
            matrix,
            &CurrentReview {
                candidate_version,
                configuration_sha256,
                pages,
                no_sold_page_recorded,
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

    /// The dealer's profile can be a review's primary page: marked as no product page, it needs no
    /// schema, is not evaluated and no check counts it.
    #[test]
    fn should_not_refuse_a_marked_primary_page_as_not_covered() {
        let product = product_page("chair", "h1", "Chair");
        let (mut profile, _) = product_page("profile", "h1", "Chair");
        let profile_html = "<html><body><h1>Simon Wharton Antiques</h1></body></html>".to_owned();
        let unmarked = [product.clone(), (profile.clone(), profile_html.clone())];
        profile.not_product_reason = Some("the dealer's profile".to_owned());
        let marked = [product, (profile, profile_html)];

        let refused = checks(
            Some(&live_matrix(
                &[schema("h1")],
                &[],
                &unmarked,
                Duration::ZERO,
            )),
            &review_pages(&unmarked),
        );
        assert!(
            refused
                .iter()
                .any(|check| matches!(check, FailedApprovalCheck::PagesNotCovered { .. })),
            "{refused:?}"
        );
        let matrix = live_matrix(&[schema("h1")], &[], &marked, Duration::ZERO);
        assert_eq!(matrix.candidates[0].pages.len(), 1);
        assert_eq!(checks(Some(&matrix), &review_pages(&marked)), Vec::new());
    }

    #[test]
    fn should_not_count_a_marked_page_as_the_sold_page() {
        let (sold, sold_html) = product_page("sold", "h1", "Sold chair");
        let mut sold = sold;
        sold.not_product_reason = Some("a gift card".to_owned());
        let pages = [
            product_page("chair", "h1", "Chair"),
            (sold, sold_html.replace("Available", "Sold")),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let refused = checks_recorded(
            Some(&matrix),
            &review_pages(&pages),
            3,
            CONFIGURATION,
            false,
        );

        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::NoSoldPageTested { .. }]
            ),
            "{refused:?}"
        );
    }

    /// A marked page shows no product, so a title it shares with a product page is no repetition.
    #[test]
    fn should_not_compare_a_marked_page_with_the_product_pages() {
        let (mut profile, profile_html) = product_page("profile", "h1", "Antique Shop");
        profile.not_product_reason = Some("the dealer's profile".to_owned());
        let pages = [
            product_page("chair", "h1", "Antique Shop"),
            (profile, profile_html),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        assert_eq!(checks(Some(&matrix), &review_pages(&pages)), Vec::new());
    }

    /// A matrix that evaluated a page marked since is outdated; marking drops the stored matrix, so
    /// only a matrix stored in a race can show it.
    #[test]
    fn should_refuse_a_live_matrix_that_evaluated_a_page_marked_since() {
        let pages = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        let mut current = review_pages(&pages);
        current[1].not_product_reason = Some("a category page".to_owned());

        let refused = checks(Some(&matrix), &current);

        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::LiveMatrixOutdated { .. }]
            ),
            "{refused:?}"
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

    /// A status slot shows "Sold" on sold pages and the amount on live ones: price text in `state`
    /// is accepted once the same rule resolves to sold on a review page, and then also tests a
    /// sold page; `auto` approval still refuses it.
    #[test]
    fn should_accept_price_text_in_a_state_that_resolves_to_sold_on_another_page() {
        let pages = [
            page_in_state("chair", "£ 3,800 plus VAT"),
            page_in_state("table", "Sold"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        assert_eq!(
            checks_recorded(
                Some(&matrix),
                &review_pages(&pages),
                3,
                CONFIGURATION,
                false
            ),
            Vec::new()
        );
        assert!(!schema_matrix_deterministic_approval_ok(&matrix));
    }

    /// Without a page on which the slot resolves to sold, price text in `state` stays refused.
    #[test]
    fn should_refuse_price_text_in_a_state_that_resolves_on_no_page() {
        let pages = [
            page_in_state("chair", "£ 3,800 plus VAT"),
            page_in_state("table", "POA"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let refused = checks_recorded(
            Some(&matrix),
            &review_pages(&pages),
            3,
            CONFIGURATION,
            false,
        );

        assert!(
            refused
                .iter()
                .any(|check| matches!(check, FailedApprovalCheck::ImplausibleExtraction { .. })),
            "{refused:?}"
        );
        assert!(
            refused
                .iter()
                .any(|check| matches!(check, FailedApprovalCheck::NoSoldPageTested { .. })),
            "{refused:?}"
        );
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

    /// The exempt price text of a status slot is no reason for the refusal, so the refusal does not
    /// list it next to the check that blocks.
    #[test]
    fn should_list_only_the_blocking_checks_of_an_implausible_page() {
        let (chair, chair_html) = product_page("chair", "h1", "Antique Shop");
        let (table, table_html) = product_page("table", "h1", "Antique Shop");
        let pages = [
            (chair, chair_html.replace("Available", "£ 3,800 plus VAT")),
            (table, table_html.replace("Available", "Sold")),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let implausible = implausible_only(&matrix, &review_pages(&pages));

        assert_eq!(implausible.len(), 2, "{implausible:?}");
        for page in &implausible {
            assert!(page.plausibility_findings.is_empty(), "{page:?}");
        }
        assert_eq!(
            FailedApprovalCheck::ImplausibleExtraction { pages: implausible }.to_string(),
            "2 extraction(s) fail a plausibility check deterministic approval refuses: \
             title REPEATED_ACROSS_PAGES on https://shop.example/products/chair; \
             title REPEATED_ACROSS_PAGES on https://shop.example/products/table"
        );
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

    const PROSE: &str = "A Georgian oak chest of drawers with original brass handles, restored in 2019 and in very good condition.";

    /// `pages` whose HTML also shows a price and a description block.
    fn with_price_and_description(
        pages: [(CrawlerReviewPage, String); 2],
    ) -> [(CrawlerReviewPage, String); 2] {
        pages.map(|(page, html)| {
            let content = format!(
                "<span class=\"price\">Price £113</span><div class=\"description\">{PROSE}</div></body>"
            );
            (page, html.replace("</body>", &content))
        })
    }

    #[test]
    fn should_name_the_pages_that_show_a_price_and_a_description_no_rule_captures() {
        let pages = with_price_and_description([
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ]);
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

        let refused = checks(Some(&matrix), &review_pages(&pages));

        assert_eq!(
            refused,
            vec![FailedApprovalCheck::ContentRuleMissing {
                rules: vec![
                    MissingContentRule {
                        field: "price",
                        pages: pages.iter().map(|(page, _)| guarded(page)).collect(),
                    },
                    MissingContentRule {
                        field: "description",
                        pages: pages.iter().map(|(page, _)| guarded(page)).collect(),
                    },
                ],
            }]
        );
    }

    #[test]
    fn should_pass_rules_for_what_the_pages_show_and_pages_that_show_nothing_to_capture() {
        let shown = with_price_and_description([
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ]);
        let mut captured = schema("h1");
        captured.price = Some(rule(".price", ExtractionKind::Text));
        captured.description = Some(rule(".description", ExtractionKind::Text));
        let matrix = live_matrix(&[captured], &[], &shown, Duration::ZERO);
        assert_eq!(checks(Some(&matrix), &review_pages(&shown)), Vec::new());

        // An auction-style shop shows its estimate in the price element.
        let mut estimated = schema("h1");
        estimated.price_estimate_min = Some(rule(".price", ExtractionKind::Text));
        estimated.description = Some(rule(".description", ExtractionKind::Text));
        let matrix = live_matrix(&[estimated], &[], &shown, Duration::ZERO);
        assert_eq!(checks(Some(&matrix), &review_pages(&shown)), Vec::new());

        let bare = [
            product_page("chair", "h1", "Chair"),
            product_page("table", "h1", "Table"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &bare, Duration::ZERO);
        assert_eq!(checks(Some(&matrix), &review_pages(&bare)), Vec::new());
    }

    /// A matrix stored before the page signals cannot show what the pages hold, so it is outdated
    /// rather than passing the content check unseen.
    #[test]
    fn should_treat_a_live_matrix_without_page_signals_as_outdated() {
        let pages = [product_page("chair", "h1", "Chair")];
        let mut matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        matrix.page_signals.clear();

        let refused = checks(Some(&matrix), &review_pages(&pages));

        assert!(
            matches!(
                refused.as_slice(),
                [FailedApprovalCheck::LiveMatrixOutdated { .. }]
            ),
            "{refused:?}"
        );
    }

    /// A page whose `state` element holds `state`.
    fn page_in_state(slug: &str, state: &str) -> (CrawlerReviewPage, String) {
        let (page, html) = product_page(slug, "h1", slug);
        (page, html.replace("Available", state))
    }

    /// An approval needs a review page that shows a sold or unavailable product, or the author's
    /// record that it found none.
    #[test]
    fn should_refuse_a_candidate_tested_on_no_sold_page_unless_the_author_recorded_none() {
        let pages = [
            page_in_state("chair", "In stock"),
            page_in_state("table", "Add to cart"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        let current = review_pages(&pages);

        assert_eq!(
            checks_recorded(Some(&matrix), &current, 3, CONFIGURATION, false),
            vec![FailedApprovalCheck::NoSoldPageTested {
                states_seen: vec![
                    SeenState {
                        url: "https://shop.example/products/chair".to_owned(),
                        state: "In stock".to_owned(),
                        resolves_to: Some("IN_STOCK"),
                        mentions: None,
                    },
                    SeenState {
                        url: "https://shop.example/products/table".to_owned(),
                        state: "Add to cart".to_owned(),
                        resolves_to: Some("AVAILABLE"),
                        mentions: None,
                    },
                ],
            }]
        );
        assert_eq!(
            checks_recorded(Some(&matrix), &current, 3, CONFIGURATION, true),
            Vec::new()
        );
    }

    /// The refusal names each state seen with what the normalization makes of it.
    #[test]
    fn should_name_the_states_seen_in_the_no_sold_page_message() {
        let pages = [
            page_in_state("chair", "Send a message to Simon Wharton Antiques"),
            page_in_state("table", "In stock"),
        ];
        let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);
        let current = review_pages(&pages);

        let message = checks_recorded(Some(&matrix), &current, 3, CONFIGURATION, false)
            .iter()
            .map(ToString::to_string)
            .collect::<String>();

        assert!(
            message.contains("'Send a message to Simon Wharton Antiques' → unresolved on https://shop.example/products/chair"),
            "{message}"
        );
        assert!(
            message.contains("'In stock' → IN_STOCK on https://shop.example/products/table"),
            "{message}"
        );
    }

    /// A sold notice written as a sentence counts as a sold page when it mentions the status; a
    /// sentence that names none, or an available label, does not.
    #[test]
    fn should_count_a_state_sentence_that_mentions_a_sold_status() {
        for (state, counts) in [
            (
                "This item has been sold, though you can still email the seller if you wish",
                true,
            ),
            ("Deze kaart is reeds verkocht.", true),
            ("Send a message to Simon Wharton Antiques", false),
            ("In stock", false),
        ] {
            let pages = [page_in_state("chair", state)];
            let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

            let refused = checks_recorded(
                Some(&matrix),
                &review_pages(&pages),
                3,
                CONFIGURATION,
                false,
            );

            assert_eq!(refused.is_empty(), counts, "{state}: {refused:?}");
        }
    }

    /// The labels the normalization resolves to sold or unavailable count, whichever language or
    /// schema.org spelling the shop uses; a cart action or an available label does not.
    #[test]
    fn should_accept_a_page_whose_state_resolves_to_sold_or_unavailable() {
        for state in [
            "Sold",
            "Verkocht",
            "https://schema.org/SoldOut",
            "Out of stock",
        ] {
            let pages = [
                page_in_state("chair", "Available"),
                page_in_state("table", state),
            ];
            let matrix = live_matrix(&[schema("h1")], &[], &pages, Duration::ZERO);

            assert_eq!(
                checks_recorded(
                    Some(&matrix),
                    &review_pages(&pages),
                    3,
                    CONFIGURATION,
                    false
                ),
                Vec::new(),
                "{state}"
            );
        }
    }

    /// A page the candidate's removed-page template captures as removed is a sold page too.
    #[test]
    fn should_accept_a_page_a_removed_page_template_captures() {
        let pages = [product_page("chair", "h1", "Chair"), notice_page("sold")];
        let template = RemovedPageSchema {
            selector: ".notice".into(),
            text: Some("no longer available".to_owned()),
            regex: None,
        };
        let matrix = live_matrix(&[schema("h1")], &[template], &pages, Duration::ZERO);

        assert_eq!(
            checks_recorded(
                Some(&matrix),
                &review_pages(&pages),
                3,
                CONFIGURATION,
                false
            ),
            Vec::new()
        );
    }

    fn pattern(value: &str) -> UrlPatternDecision {
        UrlPatternDecision::Pattern {
            value: value.to_owned(),
            exclude: None,
            matching: UrlPatternMatching::FullUrlSearch,
        }
    }

    fn excluding(value: &str, exclude: &str) -> UrlPatternDecision {
        UrlPatternDecision::Pattern {
            value: value.to_owned(),
            exclude: Some(exclude.to_owned()),
            matching: UrlPatternMatching::FullUrlSearch,
        }
    }

    /// A sample of a shop with product URLs under `/item/` and other pages besides.
    const SAMPLE: [&str; 5] = [
        "https://antique-shop.example/item/oak-chair",
        "https://antique-shop.example/item/brass-lamp/",
        "https://antique-shop.example/item-category/chairs",
        "https://antique-shop.example/shop/",
        "https://antique-shop.example/impressum",
    ];

    fn sample_checks(decision: UrlPatternDecision, sample: &[&str]) -> Vec<FailedApprovalCheck> {
        let sample: Vec<String> = sample.iter().map(|url| (*url).to_owned()).collect();
        url_pattern_approval_checks(
            &UrlPatternReviewCandidate::new(decision, None),
            "antique-shop.example",
            &sample,
        )
    }

    /// A pattern that takes some of the sample's URLs and leaves others passes without a single
    /// sample label: induction is label-blind, and the guard reads none. A trailing slash the
    /// spider stored does not keep the pattern from taking a URL.
    #[test]
    fn should_pass_a_pattern_that_takes_some_but_not_all_sample_urls_without_labels() {
        assert_eq!(sample_checks(pattern("/item/[^/]+$"), &SAMPLE), Vec::new());
    }

    #[test]
    fn should_refuse_an_undecided_url_pattern() {
        assert_eq!(
            sample_checks(UrlPatternDecision::Undecided, &SAMPLE),
            vec![FailedApprovalCheck::CandidateUndecided]
        );
    }

    /// A pattern that takes no sample URL describes no product URL the crawl found, and an empty
    /// sample shows nothing, so it is refused too.
    #[test]
    fn should_refuse_a_pattern_that_takes_no_sample_url() {
        assert_eq!(
            sample_checks(pattern("/produkt/[^/]+$"), &SAMPLE),
            vec![FailedApprovalCheck::PatternMatchesNoSampleUrl { sample_total: 5 }]
        );
        assert_eq!(
            sample_checks(pattern("/item/[^/]+$"), &[]),
            vec![FailedApprovalCheck::PatternMatchesNoSampleUrl { sample_total: 0 }]
        );
    }

    /// A pattern that takes every URL of a sample of two or more tells no product page apart; on
    /// a sample of one URL, taking it is no evidence of that.
    #[test]
    fn should_refuse_a_pattern_that_takes_every_url_of_a_sample_of_at_least_two() {
        assert_eq!(
            sample_checks(pattern("antique-shop"), &SAMPLE),
            vec![FailedApprovalCheck::PatternMatchesEverySampleUrl { sample_total: 5 }]
        );
        assert_eq!(
            sample_checks(pattern("/item/[^/]+$"), &SAMPLE[..1]),
            Vec::new()
        );
    }

    /// The decision applies to the review's crawl host alone, `www.` or not, so a sample URL on
    /// another host, such as another domain of the same shop, neither counts as taken nor makes
    /// the sample larger.
    #[test]
    fn should_count_only_sample_urls_on_the_review_crawl_host() {
        let elsewhere = SAMPLE.map(|url| url.replace("antique-shop.example", "antique-shop.de"));
        let elsewhere: Vec<&str> = elsewhere.iter().map(String::as_str).collect();
        let www_item = "https://www.antique-shop.example/item/pine-table";

        assert_eq!(
            sample_checks(pattern("/item/[^/]+$"), &elsewhere),
            vec![FailedApprovalCheck::PatternMatchesNoSampleUrl { sample_total: 0 }]
        );
        assert_eq!(
            sample_checks(
                pattern("/item/[^/]+$"),
                &[SAMPLE[0], www_item, elsewhere[2], elsewhere[3]]
            ),
            vec![FailedApprovalCheck::PatternMatchesEverySampleUrl { sample_total: 2 }]
        );
    }

    /// A URL the exclusion takes is no URL the decision takes, in either trailing-slash spelling:
    /// an exclusion of every URL the pattern takes leaves it taking none.
    #[test]
    fn should_count_only_urls_the_pattern_takes_and_the_exclusion_does_not() {
        assert_eq!(
            sample_checks(excluding("/item/[^/]+$", "oak-chair|brass-lamp/$"), &SAMPLE),
            vec![FailedApprovalCheck::PatternMatchesNoSampleUrl { sample_total: 5 }]
        );
        assert_eq!(
            sample_checks(excluding(".", "/impressum$"), &SAMPLE),
            Vec::new()
        );
    }

    /// Nothing in a review shows that a shop has no product URLs, so an agent never approves
    /// `NO_PATTERN`, whatever the sample.
    #[test]
    fn should_refuse_no_pattern_as_a_human_decision() {
        assert_eq!(
            sample_checks(UrlPatternDecision::NoPattern, &SAMPLE),
            vec![FailedApprovalCheck::NoPatternNeedsHuman]
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
            serde_json::to_value(FailedApprovalCheck::NoSoldPageTested {
                states_seen: vec![SeenState {
                    url: "https://antiques.example/products/chair".to_owned(),
                    state: "Sold".to_owned(),
                    resolves_to: None,
                    mentions: Some("SOLD_OUT"),
                }],
            })
            .ok(),
            Some(serde_json::json!({
                "check": "NO_SOLD_PAGE_TESTED",
                "states_seen": [{
                    "url": "https://antiques.example/products/chair",
                    "state": "Sold",
                    "resolves_to": null,
                    "mentions": "SOLD_OUT",
                }],
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
            serde_json::to_value(FailedApprovalCheck::PatternMatchesNoSampleUrl {
                sample_total: 20,
            })
            .ok(),
            Some(serde_json::json!({
                "check": "PATTERN_MATCHES_NO_SAMPLE_URL",
                "sample_total": 20,
            }))
        );
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::PatternMatchesEverySampleUrl {
                sample_total: 20,
            })
            .ok(),
            Some(serde_json::json!({
                "check": "PATTERN_MATCHES_EVERY_SAMPLE_URL",
                "sample_total": 20,
            }))
        );
        assert_eq!(
            serde_json::to_value(FailedApprovalCheck::NoPatternNeedsHuman).ok(),
            Some(serde_json::json!({ "check": "NO_PATTERN_NEEDS_HUMAN" }))
        );
    }
}
