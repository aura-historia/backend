//! What the review tools answer an agent: their own output types, with JSON schemas from
//! `schemars`, each mapped explicitly from the console's answer in [`super::wire`]. Lists and texts
//! that can grow are cut at fixed bounds, and every cut is named in the answer's `truncations`.

mod matrix;

pub use matrix::{MatrixAnswer, MatrixView};

use super::bounds::{keep_first, keep_last, truncated_text};
use super::wire;
use crate::review::model::{
    CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl, RepairRequest, ReviewSchemaMatrix,
    SampleLabels, SampleUrlLabel, SchemaMatrix, UrlPatternDecision, UrlPatternReviewCandidate,
};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The most pending reviews `list_pending_reviews` answers.
const MAX_PENDING_REVIEWS: usize = 50;
/// The most URLs of a URL-pattern review's sample `get_review` answers.
const MAX_SAMPLE_URLS: usize = 150;
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

/// The pending reviews of the console's queue.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PendingReviews {
    /// Pending reviews in the console's queue order: those of crawl-enabled ListingSources first,
    /// newest first within each group.
    pub reviews: Vec<PendingReview>,
    /// How many of the first 200 reviews the console lists are pending. It lists pending reviews
    /// first, so 200 means at least 200.
    pub total_pending: usize,
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
    pub listing_source_name: Option<String>,
    /// Whether the ListingSource is crawled; a review of a disabled one holds nothing back.
    pub listing_source_crawl_enabled: bool,
    /// The candidate's version, which edits and approvals name as `expected_candidate_version`.
    pub candidate_version: i64,
    /// The latest needs-repair request the review waits for, if any.
    pub repair_request: Option<RepairRequestView>,
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
    /// The domain a URL-pattern review decides.
    pub domain_id: Option<String>,
    /// The candidate's version, which edits and approvals name as `expected_candidate_version`.
    pub candidate_version: i64,
    /// Whether the review is approved and its candidate is the ListingSource's live schema list.
    pub candidate_is_live: bool,
    pub reviewer_notes: Option<String>,
    pub repair_request: Option<RepairRequestView>,
    pub candidate: CandidateView,
    /// The pages a product-schema candidate is evaluated on.
    pub pages: Vec<ReviewPageView>,
    /// The URL sample of a URL-pattern review, with how the candidate classifies each URL.
    pub sample_urls: Vec<SampleUrlView>,
    pub sample_urls_total: usize,
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
    /// The domain's product URL pattern decision and the sample labels it must follow.
    UrlPattern {
        decision: UrlPatternDecisionView,
        /// The domain's pattern when the review opened.
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
    /// The domain has no product URLs.
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
    /// How the domain's current pattern classified the URL before the review.
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
    /// The pending reviews among the console's `queue`, in its order.
    pub(super) fn of(queue: Vec<CrawlerReview>) -> Self {
        let pending: Vec<CrawlerReview> = queue
            .into_iter()
            .filter(|review| review.status == crate::review::model::STATUS_PENDING_REVIEW)
            .collect();
        let total_pending = pending.len();
        let mut truncations = Vec::new();
        let pending = keep_first(pending, MAX_PENDING_REVIEWS, "reviews", &mut truncations);
        Self {
            reviews: pending
                .into_iter()
                .map(|review| PendingReview {
                    review_id: review.review_id.to_string(),
                    artifact_type: review.artifact_type,
                    reason: review.reason,
                    listing_source_id: review.listing_source_id.to_string(),
                    listing_source_name: review.listing_source_name,
                    listing_source_crawl_enabled: review.listing_source_crawl_enabled,
                    candidate_version: review.candidate_version,
                    repair_request: review.repair_request.map(RepairRequestView::of),
                    updated: timestamp(review.updated),
                })
                .collect(),
            total_pending,
            truncations,
        }
    }
}

impl RepairRequestView {
    fn of(repair: RepairRequest) -> Self {
        Self {
            note: note(&repair.note),
            requested: timestamp(repair.requested),
        }
    }
}

impl ReviewView {
    pub(super) fn of(detail: wire::ReviewDetail, candidate: wire::Candidate) -> Self {
        let wire::ReviewDetail {
            review,
            candidate_is_live,
            pages,
            urls,
            events,
        } = detail;
        let mut truncations = Vec::new();
        let matrix = stored_matrix(&review, &pages);
        let sample_urls_total = urls.len();
        let sample_urls = keep_first(urls, MAX_SAMPLE_URLS, "sample_urls", &mut truncations);
        let events_total = events.len();
        let events = keep_last(events, MAX_EVENTS, "events", &mut truncations);
        Self {
            review_id: review.review_id.to_string(),
            status: review.status,
            artifact_type: review.artifact_type,
            reason: review.reason,
            listing_source_id: review.listing_source_id.to_string(),
            listing_source_name: review.listing_source_name,
            domain_id: review.domain_id.map(|domain_id| domain_id.to_string()),
            candidate_version: review.candidate_version,
            candidate_is_live,
            reviewer_notes: review.reviewer_notes.as_deref().map(note),
            repair_request: review.repair_request.map(RepairRequestView::of),
            candidate: CandidateView::of(candidate),
            pages: pages.into_iter().map(ReviewPageView::of).collect(),
            sample_urls: sample_urls.into_iter().map(SampleUrlView::of).collect(),
            sample_urls_total,
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
    let matrix = SchemaMatrix::stored_in(&review.validation_summary)?.ok()?;
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
            "domain_id": null,
            "artifact_type": artifact_type,
            "status": status,
            "reason": "manual_schema_required",
            "candidate_payload": candidate,
            "candidate_version": 3,
            "validation_summary": {},
            "reviewer_notes": null,
            "repair_request": null,
            "created": "2026-10-27T10:00:00Z",
            "updated": "2026-10-27T11:00:00Z",
            "reviewed": null,
        })
    }

    fn decoded<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn should_list_only_pending_reviews_in_queue_order_up_to_the_bound() {
        let mut queue: Vec<Value> = (0..60)
            .map(|_| review("PENDING_REVIEW", "PRODUCT_SCHEMA", json!({})))
            .collect();
        queue.insert(1, review("APPROVED", "PRODUCT_SCHEMA", json!({})));
        let first = queue[0]["review_id"].clone();

        let pending = PendingReviews::of(decoded(Value::Array(queue)));

        assert_eq!(pending.total_pending, 60);
        assert_eq!(pending.reviews.len(), MAX_PENDING_REVIEWS);
        assert_eq!(json!(pending.reviews[0].review_id), first);
        assert_eq!(pending.truncations, ["reviews: the first 50 of 60"]);
    }

    #[test]
    fn should_show_a_url_pattern_review_with_its_labels_latest_events_and_first_sample_urls() {
        let candidate = json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": null,
            "sample_labels": {
                "labelled_at": "2026-10-27T12:00:00Z",
                "labels": [{ "url": "https://antiques.example/item/1", "label": "PRODUCT", "reason": "one chair" }],
            },
        });
        let detail: wire::ReviewDetail = decoded(json!({
            "review": review("PENDING_REVIEW", "URL_PATTERN", candidate.clone()),
            "candidate_is_live": false,
            "pages": [],
            "urls": (0..200).map(|index| json!({
                "review_url_id": crate::CrawlerReviewUrlId::new().to_string(),
                "review_id": CrawlerReviewId::new().to_string(),
                "url": format!("https://antiques.example/item/{index}"),
                "previous_class": null,
                "current_pattern_match": null,
                "candidate_pattern_match": true,
                "candidate_class": "product",
            })).collect::<Vec<_>>(),
            "events": (0..50).map(|index| json!({
                "actor": "AGENT_AUTHOR",
                "action": "CANDIDATE_EDITED",
                "candidate_version": index,
                "notes": null,
                "created": "2026-10-27T12:00:00Z",
            })).collect::<Vec<_>>(),
        }));

        let view = ReviewView::of(detail, wire::Candidate::UrlPattern(decoded(candidate)));

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
                "sample_urls: the first 150 of 200",
                "events: the last 40 of 50"
            ])
        );
        assert_eq!(view["matrix"], Value::Null);
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
}
