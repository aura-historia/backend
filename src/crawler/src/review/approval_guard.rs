//! The deterministic checks an approval by a review agent must pass in `agent` authoring mode. A
//! check reads only what the server itself evaluated and stored, a product-schema review's live
//! matrix or a URL-pattern candidate's sample labels, never a matrix or a verdict a client sends
//! with the approval; the contract is in `docs/crawler/extraction-and-review.md` (approval guard).

use crate::CrawlerReviewPageId;
use crate::network::policy::{canonical_crawl_host, url_on_crawl_host};
use crate::review::model::{
    CrawlerReviewPage, LabelledSampleUrl, LiveMatrixEvaluation, SampleUrlLabel, SchemaMatrix,
    SchemaPageReference, UrlPatternDecision, UrlPatternReviewCandidate,
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
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::utils::url::CrawledUrl;
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
    /// Review pages that show a price or a description block while no schema applying to them
    /// has a `price` or a `description` rule.
    ContentRuleMissing { rules: Vec<MissingContentRule> },
    /// No review page shows a sold or unavailable product, and the review records no search for
    /// one that found none. `states_seen` lists what the candidate schemas extract as `state` on
    /// the pages they apply to, and what the normalization resolves each to.
    NoSoldPageTested { states_seen: Vec<SeenState> },
    /// The URL-pattern candidate decides nothing yet.
    CandidateUndecided,
    /// The decision takes clearly fewer of the sample's URLs than the share of inspected pages
    /// that describe one product suggests: a pattern too narrow for the shop's product URLs, such
    /// as one that lists category slugs one by one.
    PatternTooNarrow {
        /// The sample URLs the decision takes, out of `sample_total`.
        matched: usize,
        sample_total: usize,
        /// The sample URLs whose inspection found a page describing exactly one product, out of
        /// those inspected.
        product_pages: usize,
        inspected: usize,
        /// Some of the inspected product pages the decision does not take.
        unmatched_product_pages: Vec<String>,
    },
    /// The candidate's sample labels on the review's crawl host mark fewer than `minimum` URLs as
    /// products or fewer than `minimum` as no products, so they cannot show that the decision
    /// takes the ListingSource's product pages and leaves the rest.
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
            Self::TooFewLabels {
                product,
                not_product,
                minimum,
            } => write!(
                formatter,
                "the sample labels mark {product} URL(s) on the review's crawl host PRODUCT and {not_product} NOT_PRODUCT; an agent approval needs at least {minimum} of each, which the author agent or a human labels"
            ),
            Self::PatternTooNarrow {
                matched,
                sample_total,
                product_pages,
                inspected,
                ..
            } => write!(
                formatter,
                "the decision takes {matched} of the sample's {sample_total} URLs, but {product_pages} of the {inspected} pages inspected describe one product: it is narrower than the shop's product URLs, so use a structural pattern instead of listed categories and check the product pages it misses"
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

/// The checks an agent approval of the URL-pattern `candidate` fails, none when it may go ahead:
/// the candidate is decided, and its sample labels, which the author agent or a human set and the
/// approval rests on alone, are consistent with the decision. Only labels of URLs on `domain`, the
/// crawl host of the review's ListingSource and the one host the decision applies to, count: a
/// label on another host says nothing about it. They mark at least `min_labelled_urls` URLs as
/// products and as many as no products, and the decision classifies every `PRODUCT` URL as a
/// product page and no `NOT_PRODUCT` URL. Labels belong to the candidate, so the approved candidate
/// version names the labels it is judged on, and they do not expire. No URL is classified by
/// keywords or by its page's markup here: Claude decides what a product page is, and the crawler
/// only checks that the decision follows its labels and, from the review's `sample` of the
/// crawled URLs, that it is not clearly narrower than the product pages inspected suggest.
pub(crate) fn url_pattern_approval_checks(
    candidate: &UrlPatternReviewCandidate,
    domain: &str,
    min_labelled_urls: NonZeroU32,
    sample: &[SampleUrl],
) -> Vec<FailedApprovalCheck> {
    let pattern = match &candidate.decision {
        UrlPatternDecision::Undecided => return vec![FailedApprovalCheck::CandidateUndecided],
        decision => match decision.candidate_pattern() {
            Ok(pattern) => pattern,
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
    let not_product_pages: Vec<PageKey> = labelled(SampleUrlLabel::NotProduct)
        .filter_map(|url| page_key(&url.url))
        .collect();
    if let Some(too_narrow) = pattern_too_narrow(pattern.as_ref(), sample, &not_product_pages) {
        failed.push(too_narrow);
    }
    failed
}

/// A page as the label checks tell pages apart: its crawler page spelling, with or without its
/// trailing slash, on the crawl host with or without `www.`, on http or https.
type PageKey = (String, String, Option<String>);

fn page_key(url: &str) -> Option<PageKey> {
    let page = CrawledUrl::new(Url::parse(url).ok()?).page_spelling();
    Some((
        canonical_crawl_host(page.host_str()?),
        page.path().to_owned(),
        page.query().map(str::to_owned),
    ))
}

/// A URL of a URL-pattern review's sample, with what inspecting its page found, if it was
/// inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SampleUrl {
    pub url: String,
    /// Whether the inspected page's structured data describes exactly one schema.org `Product`;
    /// `None` for a URL nobody inspected.
    pub describes_one_product: Option<bool>,
}

/// The fewest inspected sample pages the share of product pages rests on.
const MIN_INSPECTED_PAGES: usize = 8;

/// The refusal of a `pattern` that takes less than half of the share of the sample that the
/// inspected pages suggest is product pages, and misses at least one inspected page that describes
/// one product. The share is a heuristic: inspected pages are not a random draw, and a page's
/// structured data is a hint, so the check needs [`MIN_INSPECTED_PAGES`] pages, a pattern that
/// takes under half the product share, and a missed product page as direct evidence. A page
/// labelled `NOT_PRODUCT` is no product page whatever its markup, such as a gift card a shop marks
/// up as a `Product`: the label decides, and the decision must not take it. `NO_PATTERN` takes
/// nothing by design and has no share to compare.
fn pattern_too_narrow(
    pattern: Option<&ProductUrlPattern>,
    sample: &[SampleUrl],
    not_product_pages: &[PageKey],
) -> Option<FailedApprovalCheck> {
    let pattern = pattern?;
    let takes = |url: &SampleUrl| decision_takes(Some(pattern), &url.url);
    let inspected: Vec<&SampleUrl> = sample
        .iter()
        .filter(|url| url.describes_one_product.is_some())
        .collect();
    let product_pages: Vec<&SampleUrl> = inspected
        .iter()
        .copied()
        .filter(|url| {
            url.describes_one_product == Some(true)
                && page_key(&url.url).is_none_or(|page| !not_product_pages.contains(&page))
        })
        .collect();
    let unmatched_product_pages: Vec<String> = product_pages
        .iter()
        .filter(|url| !takes(url))
        .map(|url| url.url.clone())
        .take(10)
        .collect();
    let matched = sample.iter().filter(|url| takes(url)).count();
    // matched / sample < product_pages / inspected / 2, in whole numbers.
    let narrower_than_half_the_share =
        matched * inspected.len() * 2 < product_pages.len() * sample.len();
    (inspected.len() >= MIN_INSPECTED_PAGES
        && narrower_than_half_the_share
        && !unmatched_product_pages.is_empty())
    .then_some(FailedApprovalCheck::PatternTooNarrow {
        matched,
        sample_total: sample.len(),
        product_pages: product_pages.len(),
        inspected: inspected.len(),
        unmatched_product_pages,
    })
}

/// Whether the labelled `url` is on `domain`, under the crawler's host identity, which treats
/// `www.` and the bare host as one domain.
fn is_on_domain(url: &str, domain: &str) -> bool {
    Url::parse(url).is_ok_and(|url| url_on_crawl_host(&url, domain))
}

/// Whether the decision's `pattern` takes the labelled `url` as a product page, matched in the
/// crawler's URL spelling, with or without its trailing slash, as the spider matches it;
/// `NO_PATTERN` takes none.
fn decision_takes(pattern: Option<&ProductUrlPattern>, url: &str) -> bool {
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
fn decision_takes_any_spelling(pattern: Option<&ProductUrlPattern>, url: &str) -> bool {
    let Some(pattern) = pattern else {
        return false;
    };
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    let url = CrawledUrl::new(url).as_url().clone();
    let Some(domain) = url.host_str().map(canonical_crawl_host) else {
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
        }
    }

    fn excluding(value: &str, exclude: &str) -> UrlPatternDecision {
        UrlPatternDecision::Pattern {
            value: value.to_owned(),
            exclude: Some(exclude.to_owned()),
        }
    }

    const MIN_LABELLED_URLS: NonZeroU32 = NonZeroU32::new(3).unwrap();
    /// The crawl host of the URL-pattern review, on which [`PRODUCTS`] and [`NOT_PRODUCTS`] are.
    const REVIEW_CRAWL_HOST: &str = "antique-shop.example";
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
        url_pattern_approval_checks(candidate, REVIEW_CRAWL_HOST, MIN_LABELLED_URLS, &[])
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

    /// The decision applies to the review's crawl host alone, so labels on another host, such as
    /// another domain of the same shop, say nothing about it: they do not count towards the
    /// minimum, however many there are and however well the decision follows them.
    #[test]
    fn should_count_only_labels_on_the_review_crawl_host() {
        let elsewhere =
            |urls: [&str; 3]| urls.map(|url| url.replace(REVIEW_CRAWL_HOST, "antique-shop.de"));
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

    /// Sample URLs of a shop whose products sit under `/item/` and under category paths a pattern
    /// may list one by one, `inspected` of them with what their inspection found.
    fn shop_sample(inspected: usize, describes_one_product: bool) -> Vec<SampleUrl> {
        let url = |index: usize| {
            if index < 6 {
                format!("https://antique-shop.example/item/lamp-{index}")
            } else {
                format!("https://antique-shop.example/clocks/barometers/barometer-{index}")
            }
        };
        (0..20)
            .map(|index| SampleUrl {
                url: url(index),
                describes_one_product: (index % 2 == 0 && index / 2 < inspected)
                    .then_some(describes_one_product),
            })
            .collect()
    }

    /// A pattern that takes 6 of 20 sample URLs while every inspected page describes one product,
    /// some of them under a path it misses, is narrower than the shop's product URLs.
    #[test]
    fn should_refuse_a_pattern_that_takes_clearly_fewer_urls_than_the_product_share_suggests() {
        let candidate = labelled(pattern("/item/[^/]+$"), &PRODUCTS, &NOT_PRODUCTS);

        let refused = url_pattern_approval_checks(
            &candidate,
            REVIEW_CRAWL_HOST,
            MIN_LABELLED_URLS,
            &shop_sample(10, true),
        );

        assert_eq!(
            refused,
            vec![FailedApprovalCheck::PatternTooNarrow {
                matched: 6,
                sample_total: 20,
                product_pages: 10,
                inspected: 10,
                unmatched_product_pages: [6, 8, 10, 12, 14, 16, 18]
                    .map(|index| format!(
                        "https://antique-shop.example/clocks/barometers/barometer-{index}"
                    ))
                    .to_vec(),
            }]
        );
    }

    #[test]
    fn should_pass_a_structural_pattern_and_a_sample_too_thin_or_without_missed_products() {
        let narrow = labelled(pattern("/item/[^/]+$"), &PRODUCTS, &NOT_PRODUCTS);
        let structural = labelled(
            pattern("/(item|clocks/barometers)/[^/]+$"),
            &PRODUCTS,
            &NOT_PRODUCTS,
        );
        let check = |candidate: &UrlPatternReviewCandidate, sample: &[SampleUrl]| {
            url_pattern_approval_checks(candidate, REVIEW_CRAWL_HOST, MIN_LABELLED_URLS, sample)
        };

        // The pattern takes every product page the inspection found.
        assert_eq!(check(&structural, &shop_sample(10, true)), Vec::new());
        // Too few pages were inspected to say what share of the shop is products.
        assert_eq!(check(&narrow, &shop_sample(7, true)), Vec::new());
        // The inspected pages are no products, so the pattern misses none.
        assert_eq!(check(&narrow, &shop_sample(10, false)), Vec::new());
        // No sample, as on a review nobody inspected.
        assert_eq!(check(&narrow, &[]), Vec::new());
    }

    /// A page labelled `NOT_PRODUCT` is no missed product page whatever its markup: shops mark up
    /// gift cards and vouchers as a `Product`, and the decision must leave them out. The label
    /// counts under any spelling of the page.
    #[test]
    fn should_pass_a_pattern_that_leaves_out_inspected_pages_labelled_not_product() {
        let sample = shop_sample(10, true);
        let vouchers: Vec<String> = sample
            .iter()
            .filter(|url| url.url.contains("/barometers/") && url.describes_one_product.is_some())
            .map(|url| url.url.replacen("https://", "http://www.", 1) + "/")
            .collect();
        let not_products: Vec<&str> = NOT_PRODUCTS
            .into_iter()
            .chain(vouchers.iter().map(String::as_str))
            .collect();
        let candidate = labelled(pattern("/item/[^/]+$"), &PRODUCTS, &not_products);

        assert_eq!(
            url_pattern_approval_checks(&candidate, REVIEW_CRAWL_HOST, MIN_LABELLED_URLS, &sample),
            Vec::new()
        );
    }

    const GIFT_CARD: &str = "https://antique-shop.example/item/the-antique-shop-gift-card";

    /// A gift card has the product URL shape: the pattern alone takes it, and the exclusion that
    /// takes it out of the pattern lets the decision follow its `NOT_PRODUCT` label.
    #[test]
    fn should_pass_a_not_product_url_the_exclusion_takes_and_refuse_it_without_the_exclusion() {
        let not_products = [NOT_PRODUCTS[0], NOT_PRODUCTS[1], GIFT_CARD];

        assert_eq!(
            url_checks(&labelled(pattern("/item/[^/]+$"), &PRODUCTS, &not_products)),
            vec![FailedApprovalCheck::NotProductLabelMatched {
                urls: vec![GIFT_CARD.to_owned()],
            }]
        );
        assert_eq!(
            url_checks(&labelled(
                excluding("/item/[^/]+$", "gift-card"),
                &PRODUCTS,
                &not_products
            )),
            Vec::new()
        );
    }

    /// The exclusion is matched in the crawler's spelling of the URL, with or without the
    /// trailing slash, as the pattern is.
    #[test]
    fn should_match_the_exclusion_in_either_trailing_slash_spelling() {
        let not_products = [NOT_PRODUCTS[0], NOT_PRODUCTS[1], GIFT_CARD];

        assert_eq!(
            url_checks(&labelled(
                excluding("/item/[^/]+$", "gift-card/$"),
                &PRODUCTS,
                &not_products
            )),
            Vec::new()
        );
    }

    #[test]
    fn should_refuse_an_exclusion_that_takes_a_url_labelled_product() {
        let candidate = labelled(
            excluding("/item/[^/]+$", "oak-chair|pine-table"),
            &PRODUCTS,
            &NOT_PRODUCTS,
        );

        assert_eq!(
            url_checks(&candidate),
            vec![FailedApprovalCheck::ProductLabelNotMatched {
                urls: PRODUCTS[..2].iter().map(|url| (*url).to_owned()).collect(),
            }]
        );
    }

    /// A page the exclusion takes is not one the pattern takes: an exclusion that removes the
    /// shop's product URLs leaves the pattern as narrow as one that never matched them.
    #[test]
    fn should_count_only_the_urls_the_pattern_takes_and_the_exclusion_does_not_as_taken() {
        let narrow = labelled(pattern("/item/[^/]+$"), &PRODUCTS, &NOT_PRODUCTS);
        let excluded = labelled(
            excluding("/(item|clocks/barometers)/[^/]+$", "/barometers/"),
            &PRODUCTS,
            &NOT_PRODUCTS,
        );
        let sample = shop_sample(10, true);
        let checks = |candidate| {
            url_pattern_approval_checks(candidate, REVIEW_CRAWL_HOST, MIN_LABELLED_URLS, &sample)
        };

        assert!(matches!(
            checks(&excluded).as_slice(),
            [FailedApprovalCheck::PatternTooNarrow { matched: 6, .. }]
        ));
        assert_eq!(checks(&excluded), checks(&narrow));
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
            serde_json::to_value(FailedApprovalCheck::PatternTooNarrow {
                matched: 6,
                sample_total: 20,
                product_pages: 10,
                inspected: 10,
                unmatched_product_pages: vec!["https://antiques.example/b/1".to_owned()],
            })
            .ok(),
            Some(serde_json::json!({
                "check": "PATTERN_TOO_NARROW",
                "matched": 6,
                "sample_total": 20,
                "product_pages": 10,
                "inspected": 10,
                "unmatched_product_pages": ["https://antiques.example/b/1"],
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
