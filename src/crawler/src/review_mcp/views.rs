//! What the review tools answer an agent: their own output types, with JSON schemas from
//! `schemars`, each mapped explicitly from the console's answer in [`super::wire`]. Lists and texts
//! that can grow are cut at fixed bounds, and every cut is named in the answer's `truncations`.

mod matrix;

pub use matrix::{MatrixAnswer, MatrixView};

use super::bounds::{keep_first, keep_last, truncated_text};
use super::wire;
use crate::review::model::{
    CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl, PendingReviewPage, PendingReviewSummary,
    RepairRequest, ReviewSchemaMatrix, SampleLabels, SampleUrlLabel, SourceProductUrl,
    UrlPatternDecision, UrlPatternReviewCandidate,
};
use crate::review::page_projection::PageView;
use crate::review::product_markup::JsonLdProductIds;
use crate::review::schema_evaluation::stored_schema_matrix;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::spider::classification::product_url_pattern::{UrlPatternMatching, path_and_query};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The most pending reviews one answer of `list_pending_reviews` lists.
pub(super) const MAX_PENDING_REVIEWS: usize = 50;
/// The most URLs of a URL-pattern review's sample one `get_review` answer lists.
const MAX_SAMPLE_URLS: usize = 200;
/// The most bytes the URLs of one sample page take as JSON, so a page of long URLs ends early,
/// with its `sample_urls_next_offset`, instead of being cut by the answer bound.
const MAX_SAMPLE_PAGE_BYTES: usize = 30 * 1024;
/// The most events, the latest ones, `get_review` answers. The latest needs-repair request comes
/// whole as `repair_request`, so the trail is for orientation, not for the repair.
const MAX_EVENTS: usize = 10;
/// The longest note of an event in a `get_review` answer, in characters.
const MAX_EVENT_NOTE_CHARS: usize = 400;
/// The longest notes, error messages and reasons an answer carries, in characters.
const MAX_NOTE_CHARS: usize = 1_000;
/// The longest extracted value a matrix answer carries, in characters.
pub(super) const MAX_VALUE_CHARS: usize = 300;
/// The most characters of a page projection an answer carries. Every answer stays in the agent's
/// context for the rest of its run, so a window holds what a product page's outline usually
/// needs, and `offset` reads on.
pub(super) const MAX_PROJECTION_CHARS: usize = 24_000;
/// The most bytes those characters take as a JSON string, so a window of non-Latin or
/// escape-heavy text still fits the answer bound whole, with its marker line and the `offset` to
/// read on from, instead of being cut by it.
const MAX_PROJECTION_JSON_BYTES: usize = 28 * 1024;

/// One page of the pending reviews the agents can work.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PendingReviews {
    /// Pending reviews of crawl-enabled ListingSources that do not wait for a human, oldest first:
    /// at most 50, those after the review `after` named.
    pub reviews: Vec<PendingReview>,
    /// How many pending reviews of crawl-enabled ListingSources the agents can work, on every page
    /// together.
    pub total_pending: i64,
    /// How many pending reviews of crawl-enabled ListingSources wait for a human. They are not
    /// listed: every change, verdict and page fetch of an agent on them is refused with
    /// `HUMAN_REQUIRED` until a human changes or judges them.
    pub waiting_for_human: i64,
    /// Whether reviews follow the last one listed; `after` set to its `review_id` reads on, as it
    /// does when `truncations` names the reviews list.
    pub more: bool,
    /// What this answer leaves out, if anything.
    pub truncations: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PendingReview {
    pub review_id: String,
    /// `PRODUCT_SCHEMA` or `URL_PATTERN`.
    pub artifact_type: String,
    /// Why the crawler opened the review, such as `manual_schema_required`.
    pub reason: String,
    pub listing_source_id: String,
    pub listing_source_name: String,
    /// The candidate's version, which edits and approvals name as `expected_candidate_version`.
    pub candidate_version: i64,
    /// The latest needs-repair request the review waits for, if any.
    pub repair_request: Option<RepairRequestView>,
    /// The needs-repair verdicts the agents gave the review since a human last changed or judged
    /// it.
    pub agent_rounds: u32,
    pub updated: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepairRequestView {
    pub note: String,
    pub requested: String,
}

/// One review with its candidate, pages, URL sample, stored matrix and event trail.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ReviewView {
    pub review_id: String,
    /// `PENDING_REVIEW`, `APPROVED`, `REJECTED` or another closed status.
    pub status: String,
    /// `PRODUCT_SCHEMA` or `URL_PATTERN`.
    pub artifact_type: String,
    pub reason: String,
    pub listing_source_id: String,
    pub listing_source_name: Option<String>,
    /// The host the review's ListingSource is crawled on, the only host whose URLs the review
    /// tools fetch; `None` while the source has no crawl root.
    pub crawl_host: Option<String>,
    /// The candidate's version, which edits and approvals name as `expected_candidate_version`.
    pub candidate_version: i64,
    /// Whether the review is approved and its candidate is the ListingSource's live schema list.
    pub candidate_is_live: bool,
    pub reviewer_notes: Option<String>,
    pub repair_request: Option<RepairRequestView>,
    /// The needs-repair verdicts the agents gave the review since a human last changed or judged
    /// it.
    pub agent_rounds: u32,
    /// Whether the review waits for a human: the agents' needs-repair rounds reached the cap or an
    /// agent handed it over, and every change, verdict and page fetch of an agent on it is refused
    /// with `HUMAN_REQUIRED` until a human changes or judges it. Leave such a review alone.
    pub human_required: bool,
    /// What only the first page of the URL sample carries; left out from the second page on.
    #[serde(flatten)]
    pub body: Option<ReviewBody>,
    /// One page of the URL sample of a URL-pattern review, with how the candidate classifies each
    /// URL: at most 200 URLs from `sample_urls_offset` on, in the fixed order of a hash of the
    /// URL, each as path and query.
    pub sample_urls: Vec<SampleUrlView>,
    /// The position in the whole sample of the first URL of `sample_urls`.
    pub sample_urls_offset: usize,
    /// How many URLs the whole sample holds.
    pub sample_urls_total: usize,
    /// The `offset` that reads the next page of the sample with `get_review`; null on the last
    /// page.
    pub sample_urls_next_offset: Option<usize>,
    pub truncations: Vec<String>,
}

/// What a `get_review` answer carries besides the URL sample page. Every answer stays in the
/// agent's context, so a read of a later sample page leaves it out: the agent has it from the first
/// read, and the `candidate_version` of the later answer says whether it still holds.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ReviewBody {
    pub candidate: CandidateView,
    /// The pages a product-schema candidate is evaluated on.
    pub pages: Vec<ReviewPageView>,
    /// Product URLs of the review's ListingSource that are not review pages, for picking live,
    /// sold, price-on-application or photo-less pages to test with `evaluate_candidate` or add
    /// with `add_review_page`; empty on a URL-pattern review. At most 40: those that scraped
    /// without an error first, the most recently scraped first, then the rest.
    pub source_product_urls: Vec<SourceProductUrlView>,
    /// The product-schema matrix the console stored for the candidate, if any. A candidate change
    /// drops it; `refresh_matrix` evaluates a new one.
    pub matrix: Option<MatrixView>,
    /// The latest events of the review's trail, oldest first.
    pub events: Vec<EventView>,
    pub events_total: usize,
}

/// A review's candidate.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateView {
    /// The product CSS schemas approving the review makes live, and the removed-page templates.
    ProductSchema {
        schemas: Vec<ProductCssSelectorSchema>,
        removed_page_schemas: Vec<RemovedPageSchema>,
    },
    /// The ListingSource's product URL pattern decision and the sample labels it must follow.
    UrlPattern {
        decision: UrlPatternDecisionView,
        /// The ListingSource's pattern when the review opened.
        current_pattern: Option<String>,
        /// The exclusion of the ListingSource's pattern when the review opened, when it had one.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_exclude: Option<String>,
        /// What the ListingSource's pattern was matched against when the review opened, when it
        /// had one.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_matching: Option<UrlPatternMatching>,
        sample_labels: Vec<SampleLabelView>,
        labelled_at: Option<String>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPatternDecisionView {
    /// A regex; a URL it matches is a product URL unless the exclusion, a regex matched anywhere
    /// in what `matching` names, takes it. Under PATH_AND_QUERY_FULL_MATCH the regex matches the
    /// whole path and query; under FULL_URL_SEARCH anywhere in the whole URL.
    Pattern {
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude: Option<String>,
        matching: UrlPatternMatching,
    },
    /// The ListingSource has no product URLs.
    NoPattern,
    /// No decision staged yet.
    Undecided,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SampleLabelKind {
    Product,
    NotProduct,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SampleLabelView {
    pub url: String,
    pub label: SampleLabelKind,
    pub reason: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReviewPageView {
    pub review_page_id: String,
    pub url: String,
    /// `PRIMARY`, `SEED`, `TRIGGERING_GENERATION_PAGE`, `REVIEWER_ADDED` or `WAITING_PAGE`.
    pub role: String,
    pub fetched: String,
    /// Why the page was marked as no product page; null for a page the candidate resolves. A
    /// marked page is left out of the matrix and of the coverage.
    pub not_product_reason: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceProductUrlView {
    pub url: String,
    /// When the scraper last evaluated the page; null before its first scrape.
    pub last_scraped: Option<String>,
    /// The error kind of the last scrape, such as `MissingRequiredValue`; null when it scraped
    /// without one.
    pub last_error_kind: Option<String>,
}

/// One URL of a URL-pattern review's sample. A field that is unknown, such as every pattern match
/// before the ListingSource has a pattern, is left out rather than answered as null: a sample page
/// lists up to [`MAX_SAMPLE_URLS`] of them, and each answer stays in the agent's context.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SampleUrlView {
    pub url: String,
    /// How the ListingSource's current pattern classified the URL before the review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_pattern_match: Option<bool>,
    /// Whether the candidate's pattern matches the URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_pattern_match: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_class: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct EventView {
    /// `HUMAN`, `AGENT_AUTHOR`, `AGENT_REVIEWER`, `SYSTEM` or the retired `AGENT`.
    pub actor: String,
    pub action: String,
    pub candidate_version: i64,
    pub notes: Option<String>,
    pub created: String,
}

/// A page as the review agents read it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageProjectionView {
    pub review_id: String,
    pub review_page_id: Option<String>,
    pub url: String,
    pub final_url: String,
    /// Whether the page's JSON-LD or microdata describes exactly one schema.org `Product`. A hint,
    /// never proof: the projection strips this markup with the scripts.
    pub describes_one_product: bool,
    /// The `productID` and `sku` of the page's top-level JSON-LD `Product`, null for one the page
    /// does not carry: the values a `json_ld_product` rule with property `product_id` or `sku`
    /// reads, each cut to [`MAX_VALUE_CHARS`] so the projection window keeps its share of the answer
    /// bound. The projection strips the script they are in, so they show nowhere else.
    pub json_ld_product_ids: JsonLdProductIds,
    /// The view `projection` shows: `OUTLINE` or `MARKDOWN`.
    pub view: PageView,
    /// The page in `view`, without scripts, styles, navigation, header and footer. The outline
    /// shows one element per line in CSS notation, indented by depth, with its text after ` | `:
    /// the elements, attributes and text the schema generation prompt shows, with runs of
    /// same-structure siblings counted. The markdown shows the readable content, with no tags,
    /// attributes or URLs.
    pub projection: String,
    /// The character of the whole projection this answer starts at.
    pub projection_offset: usize,
    /// The whole projection's length in characters.
    pub projection_chars: usize,
    /// Whether this answer shows only part of the projection.
    pub projection_truncated: bool,
    pub truncations: Vec<String>,
}

/// An accepted change of the candidate.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CandidateChanged {
    pub review_id: String,
    /// The candidate's version after the change; name it in the next edit or approval.
    pub candidate_version: i64,
}

/// A record that the author found no sold or unavailable page.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NoSoldPageRecorded {
    pub review_id: String,
    /// How many distinct pages the record names.
    pub recorded_urls: usize,
}

/// The label the author gave the review's ListingSource, which switched its crawling off.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceLabelled {
    pub review_id: String,
    pub label: crate::review::model::SourceLabel,
    /// When the label was stored, RFC 3339.
    pub labelled_at: String,
}

/// A page added to the review.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageAdded {
    pub review_id: String,
    pub review_page_id: String,
    /// The page's URL after redirects, as the review stores it.
    pub url: String,
    pub role: String,
}

/// A page removed from the review.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageRemoved {
    pub review_id: String,
    pub review_page_id: String,
}

/// A review page marked as no product page.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageMarkedNotProduct {
    pub review_id: String,
    pub review_page_id: String,
    pub url: String,
    /// The reason the console stored.
    pub reason: String,
}

/// A verdict the console took.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VerdictTaken {
    pub review_id: String,
    /// `NEEDS_REPAIR`, `APPROVED` or `REJECTED`.
    pub verdict: String,
}

pub(super) fn timestamp(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

pub(super) fn note(text: &str) -> String {
    truncated_text(text, MAX_NOTE_CHARS)
}

pub(super) fn value(text: &str) -> String {
    truncated_text(text, MAX_VALUE_CHARS)
}

impl PendingReviews {
    /// The console's `page` of the pending reviews the agents can work, at most
    /// [`MAX_PENDING_REVIEWS`] of them.
    pub(super) fn of(page: PendingReviewPage) -> Self {
        let mut truncations = Vec::new();
        let reviews = keep_first(
            page.reviews,
            MAX_PENDING_REVIEWS,
            "reviews",
            &mut truncations,
        );
        Self {
            reviews: reviews.into_iter().map(PendingReview::of).collect(),
            total_pending: page.totals.workable,
            waiting_for_human: page.totals.waiting_for_human,
            more: page.next_after.is_some() || !truncations.is_empty(),
            truncations,
        }
    }
}

impl PendingReview {
    fn of(review: PendingReviewSummary) -> Self {
        Self {
            review_id: review.review_id.to_string(),
            artifact_type: review.artifact_type,
            reason: review.reason,
            listing_source_id: review.listing_source_id.to_string(),
            listing_source_name: review.listing_source_name,
            candidate_version: review.candidate_version,
            repair_request: review.repair_request.map(RepairRequestView::preview),
            agent_rounds: review.agent_rounds,
            updated: timestamp(review.updated),
        }
    }
}

impl RepairRequestView {
    /// The request with its whole note: the author repairs what the reviewer asked, and a cut note
    /// loses part of it. The answer bound of [`super::bounds::bound_answer`] still applies to the
    /// answer as a whole.
    fn of(repair: RepairRequest) -> Self {
        Self {
            note: repair.note,
            requested: timestamp(repair.requested),
        }
    }

    /// The request with its note cut at [`MAX_NOTE_CHARS`], for a list of many reviews.
    fn preview(repair: RepairRequest) -> Self {
        Self {
            note: note(&repair.note),
            requested: timestamp(repair.requested),
        }
    }
}

impl ReviewView {
    /// The review as the agents read it, with the page of its URL sample that starts at
    /// `sample_offset`.
    pub(super) fn of(
        detail: wire::ReviewDetail,
        candidate: wire::Candidate,
        sample_offset: usize,
    ) -> Self {
        let wire::ReviewDetail {
            review,
            candidate_is_live,
            pages,
            urls,
            source_product_urls,
            events,
        } = detail;
        let mut truncations = Vec::new();
        let sample = SamplePage::of(
            urls,
            sample_offset,
            review.listing_source_crawl_host.as_deref(),
            &mut truncations,
        );
        let body = (sample.offset == 0).then(|| {
            ReviewBody::of(
                &review,
                candidate,
                pages,
                source_product_urls,
                events,
                &mut truncations,
            )
        });
        Self {
            review_id: review.review_id.to_string(),
            status: review.status,
            artifact_type: review.artifact_type,
            reason: review.reason,
            listing_source_id: review.listing_source_id.to_string(),
            listing_source_name: review.listing_source_name,
            crawl_host: review.listing_source_crawl_host,
            candidate_version: review.candidate_version,
            candidate_is_live,
            reviewer_notes: review.reviewer_notes.as_deref().map(note),
            repair_request: review.repair_request.map(RepairRequestView::of),
            agent_rounds: review.agent_rounds,
            human_required: review.human_required,
            body,
            sample_urls: sample.urls,
            sample_urls_offset: sample.offset,
            sample_urls_total: sample.total,
            sample_urls_next_offset: sample.next_offset,
            truncations,
        }
    }
}

impl ReviewBody {
    fn of(
        review: &CrawlerReview,
        candidate: wire::Candidate,
        pages: Vec<CrawlerReviewPage>,
        source_product_urls: Vec<SourceProductUrl>,
        events: Vec<wire::ReviewEvent>,
        truncations: &mut Vec<String>,
    ) -> Self {
        let matrix = stored_matrix(review, &pages);
        let events_total = events.len();
        let events = keep_last(events, MAX_EVENTS, "events", truncations);
        Self {
            candidate: CandidateView::of(candidate),
            pages: pages.into_iter().map(ReviewPageView::of).collect(),
            source_product_urls: source_product_urls
                .into_iter()
                .map(SourceProductUrlView::of)
                .collect(),
            matrix,
            events: events.into_iter().map(EventView::of).collect(),
            events_total,
        }
    }
}

/// The matrix the review stores, with the pages it did not evaluate as the matrix API answers
/// them; `None` without one or with one that no longer decodes, which the console refreshes.
fn stored_matrix(review: &CrawlerReview, pages: &[CrawlerReviewPage]) -> Option<MatrixView> {
    let matrix = stored_schema_matrix(&review.validation_summary)?.ok()?;
    let ReviewSchemaMatrix {
        matrix,
        not_evaluated_pages,
    } = ReviewSchemaMatrix::of(matrix, pages);
    Some(MatrixView::of(wire::ReviewMatrix {
        matrix,
        not_evaluated_pages,
    }))
}

impl CandidateView {
    fn of(candidate: wire::Candidate) -> Self {
        match candidate {
            wire::Candidate::ProductSchema(candidate) => Self::ProductSchema {
                schemas: candidate.schemas,
                removed_page_schemas: candidate.removed_page_schemas,
            },
            wire::Candidate::UrlPattern(candidate) => Self::url_pattern(candidate),
        }
    }

    fn url_pattern(candidate: UrlPatternReviewCandidate) -> Self {
        let UrlPatternReviewCandidate {
            decision,
            current_pattern,
            current_exclude,
            current_matching,
            sample_labels,
        } = candidate;
        let (sample_labels, labelled_at) = match sample_labels {
            Some(SampleLabels {
                labelled_at,
                labels,
            }) => (
                labels
                    .into_iter()
                    .map(|label| SampleLabelView {
                        url: label.url,
                        label: match label.label {
                            SampleUrlLabel::Product => SampleLabelKind::Product,
                            SampleUrlLabel::NotProduct => SampleLabelKind::NotProduct,
                        },
                        reason: note(&label.reason),
                    })
                    .collect(),
                Some(timestamp(labelled_at)),
            ),
            None => (Vec::new(), None),
        };
        Self::UrlPattern {
            decision: match decision {
                UrlPatternDecision::Pattern {
                    value,
                    exclude,
                    matching,
                } => UrlPatternDecisionView::Pattern {
                    value,
                    exclude,
                    matching,
                },
                UrlPatternDecision::NoPattern => UrlPatternDecisionView::NoPattern,
                UrlPatternDecision::Undecided => UrlPatternDecisionView::Undecided,
            },
            current_matching: current_pattern.as_ref().map(|_| current_matching),
            current_pattern,
            current_exclude,
            sample_labels,
            labelled_at,
        }
    }
}

impl ReviewPageView {
    fn of(page: CrawlerReviewPage) -> Self {
        Self {
            review_page_id: page.review_page_id.to_string(),
            url: page.url,
            role: page.role,
            fetched: timestamp(page.fetched),
            not_product_reason: page.not_product_reason.as_deref().map(note),
        }
    }
}

/// One page of a review's URL sample.
struct SamplePage {
    urls: Vec<SampleUrlView>,
    offset: usize,
    total: usize,
    next_offset: Option<usize>,
}

impl SamplePage {
    /// The agents' seed of the sample, its first [`MAX_SAMPLE_URLS`] URLs in [`sample_order`],
    /// from `offset` on within at most [`MAX_SAMPLE_PAGE_BYTES`] as JSON, naming in `truncations`
    /// what the page leaves out. The fetch-free method induces a pattern from a seed of that size,
    /// and more URLs add context cost without reliably better patterns, so the rest of the sample
    /// is never listed; the approval guard still checks a pattern against all of it.
    fn of(
        mut urls: Vec<CrawlerReviewUrl>,
        offset: usize,
        crawl_host: Option<&str>,
        truncations: &mut Vec<String>,
    ) -> Self {
        urls.sort_by_cached_key(|url| sample_order(&url.url));
        let total = urls.len();
        urls.truncate(MAX_SAMPLE_URLS);
        let seed = urls.len();
        let offset = offset.min(seed);
        let mut bytes_left = MAX_SAMPLE_PAGE_BYTES;
        let urls: Vec<SampleUrlView> = urls
            .into_iter()
            .skip(offset)
            .map(|url| SampleUrlView::of(url, crawl_host))
            .take_while(|url| {
                let bytes = serde_json::to_vec(url).map_or(0, |json| json.len() + 1);
                let fits = bytes <= bytes_left;
                bytes_left = bytes_left.saturating_sub(bytes);
                fits
            })
            .collect();
        let end = offset + urls.len();
        let next_offset = (end < seed).then_some(end);
        if offset > 0 || next_offset.is_some() || seed < total {
            truncations.push(format!(
                "sample_urls: URLs {offset} to {end} of the first {seed} of {total} in hash order"
            ));
        }
        Self {
            urls,
            offset,
            total,
            next_offset,
        }
    }
}

/// Where `url` stands in the order the agents read a sample in: by the SHA-256 of the URL, which
/// only the same URL shares. The order is a fixed pseudorandom shuffle that ignores labels, classes and the order the
/// crawl found the URLs in, so the first page is a spread over every path shape rather than the
/// crawl's first section, as the seed of an induced pattern should be, and the same sample always
/// pages the same way.
fn sample_order(url: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(url.as_bytes()).into()
}

impl SourceProductUrlView {
    fn of(url: SourceProductUrl) -> Self {
        Self {
            url: url.url,
            last_scraped: url.last_scraped.map(timestamp),
            last_error_kind: url.last_error_kind,
        }
    }
}

/// `url` without its scheme and host, as the path and query a path-and-query pattern is matched
/// against ([`path_and_query`]), when it is a URL of the crawl host (with or without `www.`); any
/// other URL stays whole. Every sample URL of a ListingSource names the
/// same host, so the host is cost in the agent's context and a literal it may copy into a pattern.
pub(super) fn relative_url(url: &str, crawl_host: Option<&str>) -> String {
    let Some(crawl_host) = crawl_host else {
        return url.to_owned();
    };
    let bare = |host: &str| {
        host.strip_prefix("www.")
            .unwrap_or(host)
            .to_ascii_lowercase()
    };
    match url::Url::parse(url) {
        Ok(parsed)
            if matches!(parsed.scheme(), "http" | "https")
                && parsed
                    .host_str()
                    .is_some_and(|host| bare(host) == bare(crawl_host)) =>
        {
            path_and_query(&parsed)
        }
        _ => url.to_owned(),
    }
}

impl SampleUrlView {
    fn of(url: CrawlerReviewUrl, crawl_host: Option<&str>) -> Self {
        Self {
            url: relative_url(&url.url, crawl_host),
            previous_class: url.previous_class,
            current_pattern_match: url.current_pattern_match,
            candidate_pattern_match: url.candidate_pattern_match,
            candidate_class: url.candidate_class,
        }
    }
}

impl EventView {
    fn of(event: wire::ReviewEvent) -> Self {
        Self {
            actor: event.actor,
            action: event.action,
            candidate_version: event.candidate_version,
            notes: event
                .notes
                .as_deref()
                .map(|notes| truncated_text(notes, MAX_EVENT_NOTE_CHARS)),
            created: event.created,
        }
    }
}

impl PageProjectionView {
    /// The projection's window from `offset` on of at most [`MAX_PROJECTION_CHARS`] characters
    /// and [`MAX_PROJECTION_JSON_BYTES`] as a JSON string, with a marker line where it starts after
    /// the beginning or stops before the end.
    pub(super) fn of(projection: wire::PageProjection, offset: usize) -> Self {
        let projection_chars = projection.projection.chars().count();
        let offset = offset.min(projection_chars);
        let mut json_bytes_left = MAX_PROJECTION_JSON_BYTES;
        let mut text: String = projection
            .projection
            .chars()
            .skip(offset)
            .take(MAX_PROJECTION_CHARS)
            .take_while(|character| {
                let bytes = json_string_bytes(*character);
                let fits = bytes <= json_bytes_left;
                if fits {
                    json_bytes_left -= bytes;
                }
                fits
            })
            .collect();
        let end = offset + text.chars().count();
        let projection_truncated = offset > 0 || end < projection_chars;
        let mut truncations = Vec::new();
        if projection_truncated {
            truncations.push(format!(
                "projection: characters {offset} to {end} of {projection_chars}"
            ));
        }
        if offset > 0 {
            text.insert_str(
                0,
                &format!("# [continued: the projection from character {offset}]\n"),
            );
        }
        if end < projection_chars {
            let view = match projection.view {
                PageView::Outline => "",
                PageView::Markdown => " and view MARKDOWN",
            };
            text.push_str(&format!(
                "\n# [truncated: {} more characters; call inspect_page with offset {end}{view} to read on]\n",
                projection_chars - end
            ));
        }
        Self {
            review_id: projection.review_id.to_string(),
            review_page_id: projection.review_page_id.map(|page_id| page_id.to_string()),
            url: projection.url,
            final_url: projection.final_url,
            describes_one_product: projection.describes_one_product,
            json_ld_product_ids: JsonLdProductIds {
                product_id: projection
                    .json_ld_product_ids
                    .product_id
                    .as_deref()
                    .map(value),
                sku: projection.json_ld_product_ids.sku.as_deref().map(value),
            },
            view: projection.view,
            projection: text,
            projection_offset: offset,
            projection_chars,
            projection_truncated,
            truncations,
        }
    }
}

/// The bytes `character` takes in a JSON string as `serde_json` writes it: escaped quotes,
/// backslashes and control characters, every other character as UTF-8.
fn json_string_bytes(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        control if control < ' ' => 6,
        other => other.len_utf8(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrawlerReviewId;
    use serde_json::{Value, json};

    fn review(status: &str, artifact_type: &str, candidate: Value) -> Value {
        json!({
            "review_id": CrawlerReviewId::new().to_string(),
            "listing_source_id": listing_source_core::ListingSourceId::new().to_string(),
            "listing_source_name": "Antiques",
            "listing_source_crawl_enabled": true,
            "listing_source_crawl_host": "antiques.example",
            "artifact_type": artifact_type,
            "status": status,
            "reason": "manual_schema_required",
            "candidate_payload": candidate,
            "candidate_version": 3,
            "validation_summary": {},
            "reviewer_notes": null,
            "repair_request": null,
            "agent_rounds": 0,
            "human_required": false,
            "created": "2026-10-27T10:00:00Z",
            "updated": "2026-10-27T11:00:00Z",
            "reviewed": null,
        })
    }

    fn decoded<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    /// A page of the console's agents' queue holding `reviews` of the workable ones.
    fn queue_page(reviews: Vec<Value>, next_after: Option<Value>) -> PendingReviewPage {
        decoded(json!({
            "reviews": reviews,
            "next_after": next_after,
            "totals": { "workable": 260, "waiting_for_human": 30 },
        }))
    }

    fn queued(agent_rounds: u32) -> Value {
        json!({
            "review_id": CrawlerReviewId::new().to_string(),
            "listing_source_id": listing_source_core::ListingSourceId::new().to_string(),
            "listing_source_name": "Antiques",
            "artifact_type": "PRODUCT_SCHEMA",
            "reason": "manual_schema_required",
            "candidate_version": 3,
            "repair_request": { "note": "title takes the shop name", "requested": "2026-10-27T11:00:00Z" },
            "agent_rounds": agent_rounds,
            "human_required": false,
            "created": "2026-10-27T10:00:00Z",
            "updated": "2026-10-27T11:00:00Z",
        })
    }

    #[test]
    fn should_list_a_page_of_the_workable_reviews_with_the_totals_of_the_whole_queue() {
        let first = queued(2);
        let last = queued(0);

        let pending = json!(PendingReviews::of(queue_page(
            vec![first.clone(), last.clone()],
            Some(last["review_id"].clone()),
        )));

        assert_eq!(pending["total_pending"], 260);
        assert_eq!(pending["waiting_for_human"], 30);
        assert_eq!(pending["more"], true);
        assert_eq!(pending["truncations"], json!([]));
        assert_eq!(pending["reviews"][0]["review_id"], first["review_id"]);
        assert_eq!(pending["reviews"][0]["agent_rounds"], 2);
        assert_eq!(
            pending["reviews"][0]["repair_request"]["note"],
            "title takes the shop name"
        );
        assert_eq!(pending["reviews"][1]["review_id"], last["review_id"]);
        assert_eq!(pending["reviews"][1].get("human_required"), None);

        let last_page = PendingReviews::of(queue_page(vec![queued(0)], None));
        assert!(!last_page.more);
    }

    #[test]
    fn should_cut_a_page_longer_than_the_bound_and_say_that_more_reviews_follow() {
        let reviews: Vec<Value> = (0..60).map(|_| queued(0)).collect();
        let first = reviews[0]["review_id"].clone();

        let pending = PendingReviews::of(queue_page(reviews, None));

        assert_eq!(pending.reviews.len(), MAX_PENDING_REVIEWS);
        assert_eq!(json!(pending.reviews[0].review_id), first);
        assert_eq!(pending.truncations, ["reviews: the first 50 of 60"]);
        assert!(pending.more);
    }

    #[test]
    fn should_show_the_agents_rounds_and_whether_a_review_waits_for_a_human() {
        let mut waiting = review("PENDING_REVIEW", "PRODUCT_SCHEMA", json!({}));
        waiting["agent_rounds"] = json!(3);
        waiting["human_required"] = json!(true);

        let view = json!(ReviewView::of(
            decoded(json!({
                "review": waiting,
                "candidate_is_live": false,
                "pages": [],
                "urls": [],
                "events": [],
            })),
            wire::Candidate::ProductSchema(wire::ProductSchemaCandidate {
                schemas: vec![],
                removed_page_schemas: vec![],
            }),
            0,
        ));

        assert_eq!(
            (&view["agent_rounds"], &view["human_required"]),
            (&json!(3), &json!(true))
        );
    }

    fn long_repair_note() -> String {
        format!("{} END", "page 1: state reads a price. ".repeat(100))
    }

    #[test]
    fn should_answer_the_whole_repair_note_of_a_review_within_the_answer_bound() {
        let note = long_repair_note();
        assert!(note.chars().count() > MAX_NOTE_CHARS);
        let mut repairing = review("PENDING_REVIEW", "PRODUCT_SCHEMA", json!({}));
        repairing["repair_request"] = json!({ "note": note, "requested": "2026-10-27T11:00:00Z" });

        let answer = json!(ReviewView::of(
            decoded(json!({
                "review": repairing,
                "candidate_is_live": false,
                "pages": [],
                "urls": [],
                "events": [],
            })),
            wire::Candidate::ProductSchema(wire::ProductSchemaCandidate {
                schemas: vec![],
                removed_page_schemas: vec![],
            }),
            0,
        ));

        assert_eq!(answer["repair_request"]["note"], json!(note));
        assert_eq!(
            crate::review_mcp::bounds::bound_answer(
                answer.clone(),
                crate::review_mcp::bounds::MAX_TOOL_OUTPUT_BYTES
            ),
            Ok(answer)
        );
    }

    #[test]
    fn should_cut_the_repair_note_in_the_list_of_pending_reviews() {
        let mut queued = queued(1);
        queued["repair_request"]["note"] = json!(long_repair_note());

        let pending = json!(PendingReviews::of(queue_page(vec![queued], None)));

        let note = pending["reviews"][0]["repair_request"]["note"]
            .as_str()
            .unwrap_or_default();
        assert!(note.contains("[truncated: 1000 of "), "{note}");
    }

    /// The exclusion of the decision and of the live pattern is shown when there is one and left
    /// out when there is none.
    #[test]
    fn should_show_the_exclusion_of_the_decision_and_of_the_current_pattern() {
        let view = |candidate: Value| {
            json!(CandidateView::of(wire::Candidate::UrlPattern(decoded(
                candidate
            ))))
        };

        let excluding = view(json!({
            "decision": { "type": "PATTERN", "value": "/item/", "exclude": "gift-card" },
            "current_pattern": "/item/",
            "current_exclude": "voucher",
        }));
        let plain = view(json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": "/item/",
        }));

        assert_eq!(
            excluding["decision"],
            json!({ "type": "PATTERN", "value": "/item/", "exclude": "gift-card", "matching": "FULL_URL_SEARCH" })
        );
        assert_eq!(excluding["current_exclude"], "voucher");
        assert_eq!(excluding["current_matching"], "FULL_URL_SEARCH");
        assert_eq!(
            plain["decision"],
            json!({ "type": "PATTERN", "value": "/item/", "matching": "FULL_URL_SEARCH" })
        );
        assert!(plain.get("current_exclude").is_none(), "{plain}");
    }

    #[test]
    fn should_show_a_url_pattern_review_with_its_labels_latest_events_and_first_sample_page() {
        let candidate = json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": null,
            "sample_labels": {
                "labelled_at": "2026-10-27T12:00:00Z",
                "labels": [{ "url": "https://antiques.example/item/1", "label": "PRODUCT", "reason": "one chair" }],
            },
        });
        let detail = url_pattern_detail(&candidate, 200, 50);

        let view = ReviewView::of(detail, wire::Candidate::UrlPattern(decoded(candidate)), 0);

        let view = json!(view);
        assert_eq!(
            view["candidate"],
            json!({
                "kind": "URL_PATTERN",
                "decision": { "type": "PATTERN", "value": "/item/", "matching": "FULL_URL_SEARCH" },
                "current_pattern": null,
                "sample_labels": [{ "url": "https://antiques.example/item/1", "label": "PRODUCT", "reason": "one chair" }],
                "labelled_at": "2026-10-27T12:00:00Z",
            })
        );
        assert_eq!(
            view["sample_urls"].as_array().map(Vec::len),
            Some(MAX_SAMPLE_URLS)
        );
        assert_eq!(
            view["sample_urls"][0],
            json!({
                "url": hash_ordered(200)[0],
                "candidate_pattern_match": true,
                "candidate_class": "product",
            }),
            "the matches unknown before the ListingSource has a pattern are left out"
        );
        assert_eq!(view["sample_urls_total"], 200);
        assert_eq!(view["events"][0]["candidate_version"], 40);
        assert_eq!(view["events_total"], 50);
        assert_eq!(view["truncations"], json!(["events: the last 10 of 50"]));
        assert_eq!(view["matrix"], Value::Null);
    }

    #[test]
    fn should_pass_the_shops_product_urls_through_with_their_scrape_outcome() {
        let candidate = json!({ "schemas": [] });
        let mut detail = url_pattern_detail(&candidate, 0, 0);
        detail.source_product_urls = decoded(json!([
            { "url": "https://antiques.example/item/1", "last_scraped": "2026-11-01T10:00:00Z", "last_error_kind": null },
            { "url": "https://antiques.example/item/2", "last_scraped": null, "last_error_kind": "MissingRequiredValue" },
        ]));

        let view = ReviewView::of(
            detail,
            wire::Candidate::ProductSchema(decoded(candidate)),
            0,
        );

        assert_eq!(
            json!(view)["source_product_urls"],
            json!([
                { "url": "https://antiques.example/item/1", "last_scraped": "2026-11-01T10:00:00Z", "last_error_kind": null },
                { "url": "https://antiques.example/item/2", "last_scraped": null, "last_error_kind": "MissingRequiredValue" },
            ])
        );
    }

    fn url_pattern_detail(candidate: &Value, urls: usize, events: usize) -> wire::ReviewDetail {
        decoded(json!({
            "review": review("PENDING_REVIEW", "URL_PATTERN", candidate.clone()),
            "candidate_is_live": false,
            "pages": [],
            "urls": (0..urls).map(|index| json!({
                "review_url_id": crate::CrawlerReviewUrlId::new().to_string(),
                "review_id": CrawlerReviewId::new().to_string(),
                "url": format!("https://antiques.example/item/{index}"),
                "previous_class": null,
                "current_pattern_match": null,
                "candidate_pattern_match": true,
                "candidate_class": "product",
            })).collect::<Vec<_>>(),
            "events": (0..events).map(|index| json!({
                "actor": "AGENT_AUTHOR",
                "action": "CANDIDATE_EDITED",
                "candidate_version": index,
                "notes": null,
                "created": "2026-10-27T12:00:00Z",
            })).collect::<Vec<_>>(),
        }))
    }

    /// The paths of the first `count` sample URLs of [`url_pattern_detail`] in the order of the
    /// SHA-256 of their URL.
    fn hash_ordered(count: usize) -> Vec<String> {
        use sha2::{Digest, Sha256};
        let mut urls: Vec<String> = (0..count)
            .map(|index| format!("https://antiques.example/item/{index}"))
            .collect();
        urls.sort_by_key(|url| Sha256::digest(url.as_bytes()));
        urls.iter()
            .map(|url| {
                url.trim_start_matches("https://antiques.example")
                    .to_owned()
            })
            .collect()
    }

    /// The sample is read in a fixed shuffle, the seed order of an induced pattern: whatever order
    /// the console answers the URLs in, and whatever their classes, the pages list them the same.
    #[test]
    fn should_list_the_sample_in_a_fixed_hash_order_whatever_the_console_s_order_and_classes() {
        let candidate = json!({
            "decision": { "type": "PATTERN", "value": "^/item/[^/?#]+$" },
            "current_pattern": null,
        });
        let first_page = |reverse: bool| {
            let mut detail = url_pattern_detail(&candidate, 300, 0);
            if reverse {
                detail.urls.reverse();
                for url in &mut detail.urls {
                    url.candidate_pattern_match = Some(false);
                    url.candidate_class = Some("other".to_owned());
                }
            }
            let view = json!(ReviewView::of(
                detail,
                wire::Candidate::UrlPattern(decoded(candidate.clone())),
                0,
            ));
            view["sample_urls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|url| url["url"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };

        let (crawl_order, reversed) = (first_page(false), first_page(true));

        assert_eq!(crawl_order, reversed);
        assert_eq!(crawl_order, hash_ordered(300)[..MAX_SAMPLE_URLS]);
        assert!(
            crawl_order.iter().any(|url| url
                .trim_start_matches("/item/")
                .parse::<usize>()
                .is_ok_and(|index| index >= MAX_SAMPLE_URLS)),
            "the first page reaches past the crawl's first 200 URLs"
        );
    }

    #[test]
    fn should_list_only_the_seed_of_a_larger_sample() {
        let candidate = json!({
            "decision": { "type": "NO_PATTERN" },
            "current_pattern": null,
        });
        let page = |offset| {
            json!(ReviewView::of(
                url_pattern_detail(&candidate, 500, 0),
                wire::Candidate::UrlPattern(decoded(candidate.clone())),
                offset,
            ))
        };

        let (seed, past_it) = (page(0), page(MAX_SAMPLE_URLS));

        let listed: Vec<&str> = seed["sample_urls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|url| url["url"].as_str())
            .collect();
        assert_eq!(
            listed,
            hash_ordered(500)[..MAX_SAMPLE_URLS],
            "the seed in hash order"
        );
        assert_eq!(
            (&seed["sample_urls_next_offset"], &seed["sample_urls_total"]),
            (&Value::Null, &json!(500))
        );
        assert_eq!(
            seed["truncations"],
            json!(["sample_urls: URLs 0 to 200 of the first 200 of 500 in hash order"])
        );
        assert_eq!(past_it["sample_urls"], json!([]), "nothing past the seed");
        assert_eq!(past_it["sample_urls_next_offset"], Value::Null);
    }

    /// The sample page after the first is what the agent reads on, so it carries the candidate's
    /// version to compare and none of what the first answer already put in the context.
    #[test]
    fn should_leave_the_review_body_out_of_every_sample_page_after_the_first() {
        let candidate = json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": null,
        });
        let page = |offset| {
            json!(ReviewView::of(
                url_pattern_detail(&candidate, 400, 5),
                wire::Candidate::UrlPattern(decoded(candidate.clone())),
                offset,
            ))
        };
        let body_fields = [
            "candidate",
            "pages",
            "source_product_urls",
            "matrix",
            "events",
            "events_total",
        ];

        let (first, second) = (page(0), page(150));

        for field in body_fields {
            assert!(first.get(field).is_some(), "{field} missing from {first}");
            assert!(second.get(field).is_none(), "{field} in {second}");
        }
        assert_eq!(second["candidate_version"], first["candidate_version"]);
        assert_eq!(second["sample_urls_offset"], 150);
        assert_eq!(
            second["truncations"],
            json!(["sample_urls: URLs 150 to 200 of the first 200 of 400 in hash order"])
        );
    }

    #[test]
    fn should_cut_the_notes_of_the_events_shorter_than_other_notes() {
        let candidate = json!({ "decision": { "type": "NO_PATTERN" }, "current_pattern": null });
        let mut detail = url_pattern_detail(&candidate, 0, 1);
        detail.events[0].notes = Some("n".repeat(MAX_NOTE_CHARS));

        let view = json!(ReviewView::of(
            detail,
            wire::Candidate::UrlPattern(decoded(candidate)),
            0,
        ));

        let notes = view["events"][0]["notes"].as_str().unwrap_or_default();
        assert!(notes.contains("[truncated: 400 of "), "{notes}");
    }

    #[test]
    fn should_end_a_sample_page_of_long_urls_before_the_byte_bound() {
        let candidate = json!({ "decision": { "type": "NO_PATTERN" }, "current_pattern": null });
        let long_path = "x".repeat(1_000);
        let mut detail = url_pattern_detail(&candidate, 150, 0);
        for url in &mut detail.urls {
            url.url.push_str(&long_path);
        }

        let view = json!(ReviewView::of(
            detail,
            wire::Candidate::UrlPattern(decoded(candidate)),
            0,
        ));

        let shown = view["sample_urls"].as_array().map_or(0, Vec::len);
        assert!(shown > 0 && shown < MAX_SAMPLE_URLS, "{shown}");
        assert_eq!(view["sample_urls_next_offset"], json!(shown));
        assert!(serde_json::to_vec(&view["sample_urls"]).unwrap().len() <= MAX_SAMPLE_PAGE_BYTES);
    }

    #[test]
    fn should_show_an_empty_page_past_the_end_of_the_sample() {
        let candidate = json!({ "decision": { "type": "NO_PATTERN" }, "current_pattern": null });

        let view = json!(ReviewView::of(
            url_pattern_detail(&candidate, 10, 0),
            wire::Candidate::UrlPattern(decoded(candidate)),
            99,
        ));

        assert_eq!(
            (
                &view["sample_urls"],
                &view["sample_urls_offset"],
                &view["sample_urls_next_offset"]
            ),
            (&json!([]), &json!(10), &Value::Null)
        );
    }

    #[test]
    fn should_list_the_urls_of_the_crawl_host_as_path_and_query() {
        let host = Some("antiques.example");

        for (url, relative) in [
            ("https://antiques.example/item/1", "/item/1"),
            (
                "http://www.antiques.example/item/1?page=2&sort=a#top",
                "/item/1?page=2&sort=a",
            ),
            ("https://antiques.example", "/"),
            ("https://antiques.example/item/1?", "/item/1"),
            ("https://antiques.example:8443/item/1", "/item/1"),
            (
                "https://other.example/item/1",
                "https://other.example/item/1",
            ),
            ("not a url", "not a url"),
        ] {
            assert_eq!(relative_url(url, host), relative, "{url}");
        }
        assert_eq!(
            relative_url("https://antiques.example/a", None),
            "https://antiques.example/a"
        );
    }

    #[test]
    fn should_answer_a_long_projection_in_marked_windows() {
        let projection = format!("main\n{}", " p | text\n".repeat(4_000));
        let view = PageView::Outline;
        let page = |offset| {
            PageProjectionView::of(
                wire::PageProjection {
                    review_id: CrawlerReviewId::new(),
                    review_page_id: None,
                    url: "https://antiques.example/item/1".to_owned(),
                    final_url: "https://antiques.example/item/1".to_owned(),
                    view,
                    projection: projection.clone(),
                    describes_one_product: true,
                    json_ld_product_ids: JsonLdProductIds::default(),
                },
                offset,
            )
        };
        let total = projection.chars().count();

        let first = page(0);
        let last = page(MAX_PROJECTION_CHARS);

        assert!(first.projection_truncated);
        assert_eq!(first.projection_chars, total);
        assert!(first.projection.starts_with("main\n"));
        assert!(
            first.projection.ends_with(&format!(
                "more characters; call inspect_page with offset {MAX_PROJECTION_CHARS} to read on]\n"
            )),
            "{}",
            &first.projection[first.projection.len() - 120..]
        );
        assert_eq!(
            first.truncations,
            [format!(
                "projection: characters 0 to {MAX_PROJECTION_CHARS} of {total}"
            )]
        );
        assert!(last.projection.starts_with(&format!(
            "# [continued: the projection from character {MAX_PROJECTION_CHARS}]\n"
        )));
        assert!(!last.projection.contains("[truncated:"));
        assert_eq!(last.projection_offset, MAX_PROJECTION_CHARS);
        assert_eq!(page(usize::MAX).projection_offset, total);

        let short = PageProjectionView::of(
            wire::PageProjection {
                review_id: CrawlerReviewId::new(),
                review_page_id: None,
                url: String::new(),
                final_url: String::new(),
                view: PageView::Outline,
                projection: "page_dsl: {}\n".to_owned(),
                describes_one_product: false,
                json_ld_product_ids: JsonLdProductIds::default(),
            },
            0,
        );
        assert!(!short.projection_truncated && short.truncations.is_empty());
        assert_eq!(short.projection, "page_dsl: {}\n");
    }

    /// The markdown is windowed as the outline is, and its read-on marker names the view, so the
    /// next call reads on in the same text.
    #[test]
    fn should_answer_a_long_markdown_view_in_the_same_windows_and_read_on_in_markdown() {
        let markdown = format!("# Chair\n{}", "A paragraph of text.\n".repeat(2_000));
        let page = |view, offset| {
            PageProjectionView::of(
                wire::PageProjection {
                    review_id: CrawlerReviewId::new(),
                    review_page_id: None,
                    url: "https://antiques.example/item/1".to_owned(),
                    final_url: "https://antiques.example/item/1".to_owned(),
                    view,
                    projection: markdown.clone(),
                    describes_one_product: false,
                    json_ld_product_ids: JsonLdProductIds::default(),
                },
                offset,
            )
        };

        let first = page(PageView::Markdown, 0);
        let as_outline = page(PageView::Outline, 0);
        let next = page(PageView::Markdown, MAX_PROJECTION_CHARS);

        assert_eq!(first.view, PageView::Markdown);
        assert_eq!(json!(first)["view"], "MARKDOWN");
        assert!(first.projection.ends_with(&format!(
            "call inspect_page with offset {MAX_PROJECTION_CHARS} and view MARKDOWN to read on]\n"
        )));
        assert_eq!(
            (first.projection_chars, first.truncations.clone()),
            (as_outline.projection_chars, as_outline.truncations.clone())
        );
        assert_eq!(
            first.projection.split("\n# [truncated:").next(),
            as_outline.projection.split("\n# [truncated:").next()
        );
        assert!(next.projection.starts_with(&format!(
            "# [continued: the projection from character {MAX_PROJECTION_CHARS}]\n"
        )));
        assert!(!next.projection.contains("[truncated:"));
    }

    #[test]
    fn should_keep_a_window_of_wide_text_whole_within_the_answer_bound_and_read_on_exactly() {
        let projection = format!("page_dsl:\n{}", "  - text: \"日本の椅子\"\n".repeat(5_000));
        let view = PageView::Outline;
        let page = |offset| {
            PageProjectionView::of(
                wire::PageProjection {
                    review_id: CrawlerReviewId::new(),
                    review_page_id: None,
                    url: "https://antiques.example/item/1".to_owned(),
                    final_url: "https://antiques.example/item/1".to_owned(),
                    view,
                    projection: projection.clone(),
                    describes_one_product: false,
                    json_ld_product_ids: JsonLdProductIds::default(),
                },
                offset,
            )
        };

        let first = page(0);
        let answer = json!(first);
        let end: usize = first
            .projection
            .rsplit("call inspect_page with offset ")
            .next()
            .and_then(|rest| rest.split(' ').next())
            .and_then(|end| end.parse().ok())
            .unwrap_or_else(|| panic!("no read-on marker: {}", first.projection));

        assert!(end < MAX_PROJECTION_CHARS, "{end}");
        assert_eq!(
            crate::review_mcp::bounds::bound_answer(
                answer.clone(),
                crate::review_mcp::bounds::MAX_TOOL_OUTPUT_BYTES
            ),
            Ok(answer)
        );
        let window: String = projection.chars().take(end).collect();
        assert!(first.projection.starts_with(&window));
        let next = page(end);
        let continued: String = projection.chars().skip(end).take(100).collect();
        assert!(
            next.projection
                .strip_prefix(&format!(
                    "# [continued: the projection from character {end}]\n"
                ))
                .is_some_and(|rest| rest.starts_with(&continued)),
            "{}",
            next.projection.chars().take(100).collect::<String>()
        );
    }

    #[test]
    fn should_cut_a_long_json_ld_product_id_so_the_projection_window_stays_whole() {
        let projection = format!("page_dsl:\n{}", "  - tag: p\n".repeat(5_000));
        let view = PageProjectionView::of(
            wire::PageProjection {
                review_id: CrawlerReviewId::new(),
                review_page_id: None,
                url: "https://antiques.example/item/1".to_owned(),
                final_url: "https://antiques.example/item/1".to_owned(),
                view: PageView::Outline,
                projection,
                describes_one_product: true,
                json_ld_product_ids: JsonLdProductIds {
                    product_id: Some("9".repeat(20_000)),
                    sku: Some("R-1".to_owned()),
                },
            },
            0,
        );

        assert_eq!(view.json_ld_product_ids.sku.as_deref(), Some("R-1"));
        assert!(
            view.json_ld_product_ids
                .product_id
                .as_deref()
                .is_some_and(|id| id.ends_with("[truncated: 300 of 20000 characters]"))
        );
        let answer = json!(view);
        assert_eq!(
            crate::review_mcp::bounds::bound_answer(
                answer.clone(),
                crate::review_mcp::bounds::MAX_TOOL_OUTPUT_BYTES
            ),
            Ok(answer)
        );
    }
}
