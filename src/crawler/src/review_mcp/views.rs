//! What the review tools answer an agent: their own output types, with JSON schemas from
//! `schemars`, each mapped explicitly from the console's answer in [`super::wire`]. Lists and texts
//! that can grow are cut at fixed bounds, and every cut is named in the answer's `truncations`.

mod matrix;

pub use matrix::{MatrixAnswer, MatrixView};

use super::bounds::{keep_first, keep_last, truncated_text};
use super::wire;
use crate::review::model::{
    CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl, PendingReviewPage, PendingReviewSummary,
    RepairRequest, ReviewSchemaMatrix, SampleLabels, SampleUrlLabel, UrlPatternDecision,
    UrlPatternReviewCandidate,
};
use crate::review::product_markup::JsonLdProductIds;
use crate::review::schema_evaluation::stored_schema_matrix;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The most pending reviews one answer of `list_pending_reviews` lists.
pub(super) const MAX_PENDING_REVIEWS: usize = 50;
/// The most URLs of a URL-pattern review's sample one `get_review` answer lists.
const MAX_SAMPLE_URLS: usize = 150;
/// The most bytes the URLs of one sample page take as JSON, so a page of long URLs ends early,
/// with its `sample_urls_next_offset`, instead of being cut by the answer bound.
const MAX_SAMPLE_PAGE_BYTES: usize = 30 * 1024;
/// The most events, the latest ones, `get_review` answers.
const MAX_EVENTS: usize = 40;
/// The longest notes, error messages and reasons an answer carries, in characters.
const MAX_NOTE_CHARS: usize = 1_000;
/// The longest extracted value a matrix answer carries, in characters.
pub(super) const MAX_VALUE_CHARS: usize = 300;
/// The most characters of a page projection an answer carries.
pub(super) const MAX_PROJECTION_CHARS: usize = 36_000;
/// The most bytes those characters take as a JSON string, so a window of non-Latin or
/// escape-heavy text still fits the answer bound whole, with its marker line and the `offset` to
/// read on from, instead of being cut by it.
const MAX_PROJECTION_JSON_BYTES: usize = 40 * 1024;

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
    /// Whether the review waits for a human: the agents' needs-repair rounds reached the cap, and
    /// every change, verdict and page fetch of an agent on it is refused with `HUMAN_REQUIRED`
    /// until a human changes or judges it. Leave such a review alone.
    pub human_required: bool,
    pub candidate: CandidateView,
    /// The pages a product-schema candidate is evaluated on.
    pub pages: Vec<ReviewPageView>,
    /// One page of the URL sample of a URL-pattern review, with how the candidate classifies each
    /// URL: at most 150 URLs from `sample_urls_offset` on.
    pub sample_urls: Vec<SampleUrlView>,
    /// The position in the whole sample of the first URL of `sample_urls`.
    pub sample_urls_offset: usize,
    /// How many URLs the whole sample holds.
    pub sample_urls_total: usize,
    /// The `offset` that reads the next page of the sample with `get_review`; null on the last
    /// page.
    pub sample_urls_next_offset: Option<usize>,
    /// The product-schema matrix the console stored for the candidate, if any. A candidate change
    /// drops it; `refresh_matrix` evaluates a new one.
    pub matrix: Option<MatrixView>,
    /// The latest events of the review's trail, oldest first.
    pub events: Vec<EventView>,
    pub events_total: usize,
    pub truncations: Vec<String>,
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
        sample_labels: Vec<SampleLabelView>,
        labelled_at: Option<String>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPatternDecisionView {
    /// A regex; a URL it matches anywhere is a product URL.
    Pattern { value: String },
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
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SampleUrlView {
    pub url: String,
    /// How the ListingSource's current pattern classified the URL before the review.
    pub previous_class: Option<String>,
    pub current_pattern_match: Option<bool>,
    /// Whether the candidate's pattern matches the URL.
    pub candidate_pattern_match: Option<bool>,
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
    /// The page as YAML: the elements, attributes and text the schema generation prompt shows,
    /// without scripts, styles, navigation, header and footer.
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
            events,
        } = detail;
        let mut truncations = Vec::new();
        let matrix = stored_matrix(&review, &pages);
        let sample = SamplePage::of(urls, sample_offset, &mut truncations);
        let events_total = events.len();
        let events = keep_last(events, MAX_EVENTS, "events", &mut truncations);
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
            candidate: CandidateView::of(candidate),
            pages: pages.into_iter().map(ReviewPageView::of).collect(),
            sample_urls: sample.urls,
            sample_urls_offset: sample.offset,
            sample_urls_total: sample.total,
            sample_urls_next_offset: sample.next_offset,
            matrix,
            events: events.into_iter().map(EventView::of).collect(),
            events_total,
            truncations,
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
                UrlPatternDecision::Pattern { value } => UrlPatternDecisionView::Pattern { value },
                UrlPatternDecision::NoPattern => UrlPatternDecisionView::NoPattern,
                UrlPatternDecision::Undecided => UrlPatternDecisionView::Undecided,
            },
            current_pattern,
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
    /// The URLs from `offset` on, at most [`MAX_SAMPLE_URLS`] of them and [`MAX_SAMPLE_PAGE_BYTES`]
    /// as JSON, naming in `truncations` the part of the sample the page shows when it is not all.
    fn of(urls: Vec<CrawlerReviewUrl>, offset: usize, truncations: &mut Vec<String>) -> Self {
        let total = urls.len();
        let offset = offset.min(total);
        let mut bytes_left = MAX_SAMPLE_PAGE_BYTES;
        let urls: Vec<SampleUrlView> = urls
            .into_iter()
            .skip(offset)
            .take(MAX_SAMPLE_URLS)
            .map(SampleUrlView::of)
            .take_while(|url| {
                let bytes = serde_json::to_vec(url).map_or(0, |json| json.len() + 1);
                let fits = bytes <= bytes_left;
                bytes_left = bytes_left.saturating_sub(bytes);
                fits
            })
            .collect();
        let end = offset + urls.len();
        let next_offset = (end < total).then_some(end);
        if offset > 0 || next_offset.is_some() {
            truncations.push(format!("sample_urls: URLs {offset} to {end} of {total}"));
        }
        Self {
            urls,
            offset,
            total,
            next_offset,
        }
    }
}

impl SampleUrlView {
    fn of(url: CrawlerReviewUrl) -> Self {
        Self {
            url: url.url,
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
            notes: event.notes.as_deref().map(note),
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
            text.push_str(&format!(
                "\n# [truncated: {} more characters; call inspect_page with offset {end} to read on]\n",
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
                "decision": { "type": "PATTERN", "value": "/item/" },
                "current_pattern": null,
                "sample_labels": [{ "url": "https://antiques.example/item/1", "label": "PRODUCT", "reason": "one chair" }],
                "labelled_at": "2026-10-27T12:00:00Z",
            })
        );
        assert_eq!(
            view["sample_urls"].as_array().map(Vec::len),
            Some(MAX_SAMPLE_URLS)
        );
        assert_eq!(view["sample_urls_total"], 200);
        assert_eq!(view["events"][0]["candidate_version"], 10);
        assert_eq!(view["events_total"], 50);
        assert_eq!(
            view["truncations"],
            json!([
                "sample_urls: URLs 0 to 150 of 200",
                "events: the last 40 of 50"
            ])
        );
        assert_eq!(view["matrix"], Value::Null);
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

    #[test]
    fn should_page_through_the_whole_sample_by_offset() {
        let candidate = json!({
            "decision": { "type": "NO_PATTERN" },
            "current_pattern": null,
        });
        let page = |offset| {
            json!(ReviewView::of(
                url_pattern_detail(&candidate, 400, 0),
                wire::Candidate::UrlPattern(decoded(candidate.clone())),
                offset,
            ))
        };

        let pages: Vec<Value> = [0, 150, 300].map(page).into();

        let shown: Vec<(u64, u64, Value, u64)> = pages
            .iter()
            .map(|view| {
                (
                    view["sample_urls_offset"].as_u64().unwrap_or(u64::MAX),
                    view["sample_urls"].as_array().map_or(0, Vec::len) as u64,
                    view["sample_urls_next_offset"].clone(),
                    view["sample_urls_total"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (0, 150, json!(150), 400),
                (150, 150, json!(300), 400),
                (300, 100, Value::Null, 400)
            ]
        );
        assert_eq!(
            pages[2]["sample_urls"][0]["url"],
            "https://antiques.example/item/300"
        );
        assert_eq!(
            pages[2]["truncations"],
            json!(["sample_urls: URLs 300 to 400 of 400"])
        );
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
    fn should_answer_a_long_projection_in_marked_windows() {
        let projection = format!("page_dsl:\n{}", "  - tag: p\n".repeat(5_000));
        let page = |offset| {
            PageProjectionView::of(
                wire::PageProjection {
                    review_id: CrawlerReviewId::new(),
                    review_page_id: None,
                    url: "https://antiques.example/item/1".to_owned(),
                    final_url: "https://antiques.example/item/1".to_owned(),
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
        assert!(first.projection.starts_with("page_dsl:"));
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
                projection: "page_dsl: {}\n".to_owned(),
                describes_one_product: false,
                json_ld_product_ids: JsonLdProductIds::default(),
            },
            0,
        );
        assert!(!short.projection_truncated && short.truncations.is_empty());
        assert_eq!(short.projection, "page_dsl: {}\n");
    }

    #[test]
    fn should_keep_a_window_of_wide_text_whole_within_the_answer_bound_and_read_on_exactly() {
        let projection = format!("page_dsl:\n{}", "  - text: \"日本の椅子\"\n".repeat(5_000));
        let page = |offset| {
            PageProjectionView::of(
                wire::PageProjection {
                    review_id: CrawlerReviewId::new(),
                    review_page_id: None,
                    url: "https://antiques.example/item/1".to_owned(),
                    final_url: "https://antiques.example/item/1".to_owned(),
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
