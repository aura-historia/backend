//! What the runner reads from the review console with the author agent's token: the pending
//! reviews the agents work, in the console's queue order, and the event trail of all reviews since
//! a time, for the digest. The console decides what is pending; the runner only picks which
//! reviews this pass works.

use crate::CrawlerReviewId;
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, ARTIFACT_URL_PATTERN, CrawlerReview, STATUS_PENDING_REVIEW,
};
use crate::review_mcp::{ConsoleClient, ConsoleError};
use serde::Deserialize;
use std::collections::BTreeMap;
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

/// The pending reviews of the console's queue the agents can work: product-schema and URL-pattern
/// reviews of crawl-enabled ListingSources, in the console's order. A review of a disabled
/// ListingSource holds nothing back, so it waits until the source is crawled again.
pub async fn pending_reviews(console: &ConsoleClient) -> Result<Vec<QueuedReview>, ConsoleError> {
    let queue: Vec<CrawlerReview> = console.get("/api/reviews").await?.decode()?;
    Ok(queue
        .into_iter()
        .filter(|review| review.status == STATUS_PENDING_REVIEW)
        .filter(|review| review.listing_source_crawl_enabled)
        .filter_map(|review| {
            Some(QueuedReview {
                review_id: review.review_id,
                artifact_type: ReviewArtifact::parse(&review.artifact_type)?,
                listing_source_name: review.listing_source_name,
            })
        })
        .collect())
}

/// The first `max` of `queue` whose cooldown, if any, has passed by `now`.
pub fn select_reviews(
    queue: Vec<QueuedReview>,
    cooldowns: &BTreeMap<CrawlerReviewId, OffsetDateTime>,
    now: OffsetDateTime,
    max: usize,
) -> (Vec<QueuedReview>, usize) {
    let (cooling, ready): (Vec<_>, Vec<_>) = queue.into_iter().partition(|review| {
        cooldowns
            .get(&review.review_id)
            .is_some_and(|until| *until > now)
    });
    (ready.into_iter().take(max).collect(), cooling.len())
}

/// `GET /api/review-events`: the events of every review since a time.
#[derive(Debug, Deserialize)]
pub struct ReviewEventsSince {
    pub events: Vec<ReviewEventEntry>,
    /// Whether the console left out later events beyond its limit.
    pub truncated: bool,
}

/// One event of `GET /api/review-events`, with the keys the digest reads; actor and action are
/// the console's names, such as `AGENT_REVIEWER` and `APPROVED`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReviewEventEntry {
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

/// The events of every review since `since`, oldest first.
pub async fn review_events_since(
    console: &ConsoleClient,
    since: OffsetDateTime,
) -> Result<ReviewEventsSince, ConsoleError> {
    // RFC 3339 of a valid `OffsetDateTime` always formats.
    let since = since.format(&Rfc3339).unwrap_or_default();
    console
        .get(&format!(
            "/api/review-events?since={}",
            url::form_urlencoded::byte_serialize(since.as_bytes()).collect::<String>()
        ))
        .await?
        .decode()
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

        let (selected, cooling) = select_reviews(queue.clone(), &cooldowns, now, 3);

        assert_eq!(
            selected,
            vec![queue[1].clone(), queue[2].clone(), queue[3].clone()]
        );
        assert_eq!(cooling, 1);
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
