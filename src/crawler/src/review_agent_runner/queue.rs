//! What the runner reads from the review console with the author agent's token: every page of the
//! agents' queue (`GET /api/pending-reviews`), the pending reviews the agents work and those that
//! wait for a human, whether a review waits for a human now, and the event trail of all reviews
//! since a time, for the digest. The console decides what is pending and what waits for a human;
//! the runner only picks which reviews this pass works.

use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, ARTIFACT_URL_PATTERN, CrawlerReview, PendingReviewPage,
    PendingReviewSummary, RepairRequest,
};
use crate::review_mcp::{ConsoleClient, ConsoleError};
use crate::{CrawlerReviewEventId, CrawlerReviewId};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// What a review asks the agents to author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewArtifact {
    ProductSchema,
    UrlPattern,
}

impl ReviewArtifact {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductSchema => ARTIFACT_PRODUCT_SCHEMA,
            Self::UrlPattern => ARTIFACT_URL_PATTERN,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::ProductSchema, Self::UrlPattern]
            .into_iter()
            .find(|artifact| artifact.as_str() == value)
    }
}

impl fmt::Display for ReviewArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A pending review this pass works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedReview {
    pub review_id: CrawlerReviewId,
    pub artifact_type: ReviewArtifact,
    pub listing_source_name: Option<String>,
}

/// A pending review that waits for a human: the agents' needs-repair rounds on it reached the
/// console's cap, so no agent process works it until a human hands it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanRequiredReview {
    pub review_id: CrawlerReviewId,
    pub artifact_type: ReviewArtifact,
    pub listing_source_name: Option<String>,
    /// The agents' needs-repair verdicts since a human last changed or judged the review.
    pub agent_rounds: u32,
    /// The latest needs-repair verdict, the one that reached the cap.
    pub repair_request: Option<RepairRequest>,
}

/// Every pending product-schema and URL-pattern review of crawl-enabled ListingSources, oldest
/// first: those the agents can work and those that wait for a human. A review of a disabled
/// ListingSource holds nothing back, so it waits until the source is crawled again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewQueue {
    pub workable: Vec<QueuedReview>,
    pub waiting_for_human: Vec<HumanRequiredReview>,
}

/// The agents' queue: every page of the reviews they can work, then every page of those that wait
/// for a human, so however many wait for a human, none takes a workable review's place.
pub async fn review_queue(console: &ConsoleClient) -> Result<ReviewQueue, ConsoleError> {
    let workable = pending_reviews(console, false).await?;
    let waiting_for_human = pending_reviews(console, true).await?;
    Ok(ReviewQueue::of(workable, waiting_for_human))
}

/// Every page of the agents' queue whose reviews' `human_required` is `human_required`.
async fn pending_reviews(
    console: &ConsoleClient,
    human_required: bool,
) -> Result<Vec<PendingReviewSummary>, ConsoleError> {
    every_page(|after| async move {
        let mut path = format!("/api/pending-reviews?human_required={human_required}");
        if let Some(after) = after {
            path.push_str(&format!("&after={after}"));
        }
        console.get(&path).await?.decode()
    })
    .await
}

/// The reviews of every page `page` answers, from the first on: each page after the review the
/// previous one names in `next_after`. Each review is kept once, and a page that adds none ends
/// the read, so a console that repeats a page cannot keep the runner reading.
async fn every_page<Page, Fut>(mut page: Page) -> Result<Vec<PendingReviewSummary>, ConsoleError>
where
    Page: FnMut(Option<CrawlerReviewId>) -> Fut,
    Fut: Future<Output = Result<PendingReviewPage, ConsoleError>>,
{
    let mut reviews: Vec<PendingReviewSummary> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut after = None;
    loop {
        let answer = page(after).await?;
        let before = reviews.len();
        reviews.extend(
            answer
                .reviews
                .into_iter()
                .filter(|review| seen.insert(review.review_id)),
        );
        match answer.next_after {
            Some(next) if reviews.len() > before => after = Some(next),
            _ => return Ok(reviews),
        }
    }
}

impl ReviewQueue {
    /// The queue of the `workable` reviews and of those `waiting_for_human`, read after them: a
    /// review that came to wait for a human between the two reads waits for one.
    fn of(
        workable: Vec<PendingReviewSummary>,
        waiting_for_human: Vec<PendingReviewSummary>,
    ) -> Self {
        let waiting_ids: BTreeSet<CrawlerReviewId> = waiting_for_human
            .iter()
            .map(|review| review.review_id)
            .collect();
        Self {
            workable: workable
                .into_iter()
                .filter(|review| !waiting_ids.contains(&review.review_id))
                .filter_map(|review| {
                    Some(QueuedReview {
                        review_id: review.review_id,
                        artifact_type: ReviewArtifact::parse(&review.artifact_type)?,
                        listing_source_name: Some(review.listing_source_name),
                    })
                })
                .collect(),
            waiting_for_human: waiting_for_human
                .into_iter()
                .filter_map(|review| {
                    Some(HumanRequiredReview {
                        review_id: review.review_id,
                        artifact_type: ReviewArtifact::parse(&review.artifact_type)?,
                        listing_source_name: Some(review.listing_source_name),
                        agent_rounds: review.agent_rounds,
                        repair_request: review.repair_request,
                    })
                })
                .collect(),
        }
    }
}

/// `GET /api/reviews/{id}`, of which the runner reads only the review.
#[derive(Debug, Deserialize)]
struct ReviewDetail {
    review: CrawlerReview,
}

/// Whether the console says `review_id` waits for a human now: the agents' needs-repair rounds
/// on it reached the cap, so it refuses every agent change and verdict on it.
pub async fn waits_for_human(
    console: &ConsoleClient,
    review_id: CrawlerReviewId,
) -> Result<bool, ConsoleError> {
    let detail: ReviewDetail = console
        .get(&format!("/api/reviews/{review_id}"))
        .await?
        .decode()?;
    Ok(detail.review.human_required)
}

/// The `max` reviews of `queue` this pass works, and how many of the others are cooling down.
/// A review whose cooldown has not passed by `now` waits. Of the others, those no agent process
/// worked within the ledger's window come first, in the console's order, then those worked
/// longest ago, by `last_worked`: a review handed back and forth with needs-repair verdicts never
/// keeps the rest of the queue waiting.
pub fn select_reviews(
    queue: Vec<QueuedReview>,
    cooldowns: &BTreeMap<CrawlerReviewId, OffsetDateTime>,
    last_worked: &BTreeMap<CrawlerReviewId, OffsetDateTime>,
    now: OffsetDateTime,
    max: usize,
) -> (Vec<QueuedReview>, usize) {
    let (cooling, mut ready): (Vec<_>, Vec<_>) = queue.into_iter().partition(|review| {
        cooldowns
            .get(&review.review_id)
            .is_some_and(|until| *until > now)
    });
    ready.sort_by_key(|review| last_worked.get(&review.review_id).copied());
    (ready.into_iter().take(max).collect(), cooling.len())
}

/// The most pages of `GET /api/review-events` one digest reads.
const MAX_EVENT_PAGES: usize = 50;

/// `GET /api/review-events`: the events of every review since a time.
#[derive(Debug, Deserialize)]
pub struct ReviewEventsSince {
    pub events: Vec<ReviewEventEntry>,
    /// Whether later events were left out: by the console beyond its limit for one page, or by
    /// the runner beyond [`MAX_EVENT_PAGES`].
    pub truncated: bool,
}

/// One event of `GET /api/review-events`, with the keys the digest reads; actor and action are
/// the console's names, such as `AGENT_REVIEWER` and `APPROVED`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReviewEventEntry {
    pub event_id: CrawlerReviewEventId,
    pub review_id: CrawlerReviewId,
    pub artifact_type: String,
    pub listing_source_name: Option<String>,
    pub actor: String,
    pub action: String,
    pub candidate_version: i64,
    pub notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// The events of every review since `since`, oldest first, read page by page.
pub async fn review_events_since(
    console: &ConsoleClient,
    since: OffsetDateTime,
) -> Result<ReviewEventsSince, ConsoleError> {
    all_pages(since, |from| async move {
        // RFC 3339 of a valid `OffsetDateTime` always formats.
        let from = from.format(&Rfc3339).unwrap_or_default();
        console
            .get(&format!(
                "/api/review-events?since={}",
                url::form_urlencoded::byte_serialize(from.as_bytes()).collect::<String>()
            ))
            .await?
            .decode()
    })
    .await
}

/// The events of all pages `page` answers from `since` on: a page that left later events out is
/// followed by the page from its last event's time, which lists the events of that time again,
/// so each event is kept once, by its id. Stops after [`MAX_EVENT_PAGES`] pages, or when a page
/// adds nothing, with `truncated` set.
async fn all_pages<Page, Fut>(
    since: OffsetDateTime,
    mut page: Page,
) -> Result<ReviewEventsSince, ConsoleError>
where
    Page: FnMut(OffsetDateTime) -> Fut,
    Fut: Future<Output = Result<ReviewEventsSince, ConsoleError>>,
{
    let mut events: Vec<ReviewEventEntry> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut from = since;
    for _ in 0..MAX_EVENT_PAGES {
        let answer = page(from).await?;
        let before = events.len();
        events.extend(
            answer
                .events
                .into_iter()
                .filter(|event| seen.insert(event.event_id)),
        );
        if !answer.truncated {
            return Ok(ReviewEventsSince {
                events,
                truncated: false,
            });
        }
        match events.last() {
            Some(last) if events.len() > before => from = last.created,
            _ => break,
        }
    }
    Ok(ReviewEventsSince {
        events,
        truncated: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn queued(count: usize) -> Vec<QueuedReview> {
        (0..count)
            .map(|_| QueuedReview {
                review_id: CrawlerReviewId::new(),
                artifact_type: ReviewArtifact::ProductSchema,
                listing_source_name: None,
            })
            .collect()
    }

    #[test]
    fn should_pick_the_first_reviews_in_queue_order_whose_cooldown_has_passed() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(10_000);
        let queue = queued(5);
        let cooldowns = BTreeMap::from([
            (queue[0].review_id, now + Duration::minutes(1)),
            (queue[2].review_id, now - Duration::minutes(1)),
        ]);

        let (selected, cooling) =
            select_reviews(queue.clone(), &cooldowns, &BTreeMap::new(), now, 3);

        assert_eq!(
            selected,
            vec![queue[1].clone(), queue[2].clone(), queue[3].clone()]
        );
        assert_eq!(cooling, 1);
    }

    #[test]
    fn should_work_the_reviews_no_agent_worked_first_then_those_worked_longest_ago() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(10_000);
        let queue = queued(4);
        let last_worked = BTreeMap::from([
            (queue[0].review_id, now - Duration::minutes(30)),
            (queue[1].review_id, now - Duration::hours(5)),
        ]);

        let (selected, _) = select_reviews(queue.clone(), &BTreeMap::new(), &last_worked, now, 3);

        assert_eq!(
            selected,
            vec![queue[2].clone(), queue[3].clone(), queue[1].clone()]
        );
    }

    fn summary(human_required: bool) -> PendingReviewSummary {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::days(10_000);
        PendingReviewSummary {
            review_id: CrawlerReviewId::new(),
            listing_source_id: listing_source_core::ListingSourceId::new(),
            listing_source_name: "Antiques".to_owned(),
            domain_id: None,
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            reason: "manual_schema_required".to_owned(),
            candidate_version: 1,
            repair_request: None,
            agent_rounds: if human_required { 3 } else { 0 },
            human_required,
            created: now,
            updated: now,
        }
    }

    /// A console that answers `reviews` in pages of `page_size`, each after the review its
    /// `after` names, as `GET /api/pending-reviews` does, and records each `after` it was asked for.
    fn paged_queue(
        reviews: &[PendingReviewSummary],
        page_size: usize,
        asked: &std::sync::Mutex<Vec<Option<CrawlerReviewId>>>,
        after: Option<CrawlerReviewId>,
    ) -> Result<PendingReviewPage, ConsoleError> {
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(after);
        let start = after.map_or(0, |after| {
            reviews
                .iter()
                .position(|review| review.review_id == after)
                .map_or(reviews.len(), |index| index + 1)
        });
        let page: Vec<PendingReviewSummary> = reviews
            .iter()
            .skip(start)
            .take(page_size)
            .cloned()
            .collect();
        let next_after = (start + page.len() < reviews.len())
            .then(|| page.last().map(|review| review.review_id))
            .flatten();
        Ok(PendingReviewPage {
            reviews: page,
            next_after,
            totals: crate::review::model::PendingReviewTotals {
                workable: i64::try_from(reviews.len()).unwrap_or(i64::MAX),
                waiting_for_human: 0,
            },
        })
    }

    #[tokio::test]
    async fn should_read_every_page_of_the_queue_after_the_review_the_previous_one_ended_with() {
        let reviews: Vec<PendingReviewSummary> = (0..5).map(|_| summary(false)).collect();
        let asked = std::sync::Mutex::new(Vec::new());

        let read = every_page(|after| {
            let answer = paged_queue(&reviews, 2, &asked, after);
            async move { answer }
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(read, reviews);
        assert_eq!(
            asked.into_inner().unwrap_or_default(),
            [None, Some(reviews[1].review_id), Some(reviews[3].review_id)]
        );
    }

    #[tokio::test]
    async fn should_stop_reading_the_queue_when_a_page_adds_no_review() {
        let reviews: Vec<PendingReviewSummary> = (0..3).map(|_| summary(false)).collect();
        let asked = std::sync::Mutex::new(Vec::new());

        // A console that answers the first page whatever it is asked for.
        let read = every_page(|_| {
            let answer = paged_queue(&reviews, 2, &asked, None);
            async move { answer }
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(read, reviews[..2]);
        assert_eq!(asked.into_inner().unwrap_or_default().len(), 2);
    }

    #[test]
    fn should_keep_a_review_that_came_to_wait_for_a_human_between_the_reads_from_the_agents() {
        let workable = summary(false);
        let handed_over = summary(false);
        let waiting = summary(true);
        let handed_over_now = PendingReviewSummary {
            agent_rounds: 3,
            human_required: true,
            ..handed_over.clone()
        };

        let queue = ReviewQueue::of(
            vec![workable.clone(), handed_over],
            vec![waiting.clone(), handed_over_now.clone()],
        );

        assert_eq!(
            queue
                .workable
                .iter()
                .map(|review| review.review_id)
                .collect::<Vec<_>>(),
            [workable.review_id]
        );
        assert_eq!(
            queue
                .waiting_for_human
                .iter()
                .map(|review| (review.review_id, review.agent_rounds))
                .collect::<Vec<_>>(),
            [(waiting.review_id, 3), (handed_over_now.review_id, 3)]
        );
    }

    fn event_at(created: OffsetDateTime) -> ReviewEventEntry {
        ReviewEventEntry {
            event_id: CrawlerReviewEventId::new(),
            review_id: CrawlerReviewId::new(),
            artifact_type: "URL_PATTERN".to_owned(),
            listing_source_name: None,
            actor: "AGENT_REVIEWER".to_owned(),
            action: "APPROVED".to_owned(),
            candidate_version: 2,
            notes: None,
            created,
        }
    }

    /// A console that answers at most `limit` of `events` from a time on, oldest first, as
    /// `GET /api/review-events` does, and records each time it was asked for.
    fn paged_console(
        events: &[ReviewEventEntry],
        limit: usize,
        asked: &std::sync::Mutex<Vec<OffsetDateTime>>,
        from: OffsetDateTime,
    ) -> Result<ReviewEventsSince, ConsoleError> {
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(from);
        let later: Vec<ReviewEventEntry> = events
            .iter()
            .filter(|event| event.created >= from)
            .cloned()
            .collect();
        Ok(ReviewEventsSince {
            truncated: later.len() > limit,
            events: later.into_iter().take(limit).collect(),
        })
    }

    #[tokio::test]
    async fn should_read_every_page_of_the_events_and_keep_each_event_once() {
        let start = OffsetDateTime::UNIX_EPOCH + Duration::days(10_000);
        // Two events share each time, so a page boundary falls between events of one time.
        let events: Vec<ReviewEventEntry> = (0..7)
            .map(|index| event_at(start + Duration::seconds(index / 2)))
            .collect();
        let asked = std::sync::Mutex::new(Vec::new());

        let read = all_pages(start, |from| {
            let answer = paged_console(&events, 3, &asked, from);
            async move { answer }
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert!(!read.truncated);
        assert_eq!(read.events, events);
        assert_eq!(
            asked.into_inner().unwrap_or_default(),
            [
                start,
                start + Duration::seconds(1),
                start + Duration::seconds(2)
            ]
        );
    }

    #[tokio::test]
    async fn should_say_events_are_missing_when_a_page_adds_nothing() {
        let start = OffsetDateTime::UNIX_EPOCH + Duration::days(10_000);
        let events: Vec<ReviewEventEntry> = (0..5).map(|_| event_at(start)).collect();
        let asked = std::sync::Mutex::new(Vec::new());

        let read = all_pages(start, |from| {
            let answer = paged_console(&events, 3, &asked, from);
            async move { answer }
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert!(read.truncated);
        assert_eq!(read.events, events[..3]);
        assert_eq!(asked.into_inner().unwrap_or_default().len(), 2);
    }

    #[test]
    fn should_name_only_the_artifacts_the_agents_author() {
        assert_eq!(
            ReviewArtifact::parse("PRODUCT_SCHEMA"),
            Some(ReviewArtifact::ProductSchema)
        );
        assert_eq!(
            ReviewArtifact::parse("URL_PATTERN"),
            Some(ReviewArtifact::UrlPattern)
        );
        assert_eq!(ReviewArtifact::parse("REMOVED_PAGE"), None);
    }
}
