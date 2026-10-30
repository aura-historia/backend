//! The review agents' queue: the pending reviews of crawl-enabled ListingSources, oldest first, read
//! page by page, all of them or only those that do or do not wait for a human, with the totals of
//! the whole queue counted in SQL. Oldest first, newer reviews never keep the longest-waiting one
//! from the agents, and a review opened while a reader pages through joins the end. A page starts
//! after the review the previous one ended with, by that review's creation time and id, which
//! never change, so a review that closes or comes to wait for a human between two pages moves no
//! other review across the page boundary. The contract is in `docs/crawler/operations.md` (review
//! service).

use super::repair_rounds::{agent_round_binds, agent_rounds_of_review};
use super::{
    ReviewRepositoryError, crawler_domain_id_from_uuid, crawler_review_id_from_uuid,
    listing_source_id_from_uuid, row_to_repair_request,
};
use crate::CrawlerReviewId;
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{
    PendingReviewPage, PendingReviewSummary, PendingReviewTotals, STATUS_PENDING_REVIEW,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

/// How many reviews one page of the agents' queue holds, from 1 to [`PendingReviewPageSize::MAX`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingReviewPageSize(usize);

impl PendingReviewPageSize {
    /// The page size when none is asked for.
    pub const DEFAULT: Self = Self(100);
    /// The largest page.
    pub const MAX: Self = Self(500);

    /// Pages of `size` reviews, or `None` when `size` is not between 1 and [`Self::MAX`].
    pub fn new(size: usize) -> Option<Self> {
        (1..=Self::MAX.0).contains(&size).then_some(Self(size))
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// Which page of the agents' queue to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingReviewQuery {
    /// Only the reviews whose `human_required` is this; `None` lists both.
    pub human_required: Option<bool>,
    /// The review the previous page ended with; `None` reads the first page.
    pub after: Option<CrawlerReviewId>,
    pub page_size: PendingReviewPageSize,
}

impl Default for PendingReviewQuery {
    /// The first page of the whole queue.
    fn default() -> Self {
        Self {
            human_required: None,
            after: None,
            page_size: PendingReviewPageSize::DEFAULT,
        }
    }
}

/// The totals of the agents' queue and one page of it. A pending review waits for a human once its
/// agent rounds reach the repair cap, as [`AgentDecisionPolicy::waits_for_human`] decides. Binds
/// `$1` to `$3` of [`agent_rounds_of_review`], `$4`, the repair cap, `$5`, the `human_required`
/// filter or null, `$6` and `$7`, the creation time and id of the review the page starts after or
/// null, and `$8`, the most rows to answer. Every row carries the totals; an empty page is one row
/// with the totals and no review.
const PENDING_REVIEW_PAGE_QUERY: &str = concat!(
    "WITH queue AS ( \
         SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, \
                r.artifact_type, r.reason, r.candidate_version, r.repair_requested, \
                r.repair_note, r.created, r.updated, ",
    agent_rounds_of_review!(),
    " AS agent_rounds \
         FROM crawler_reviews r \
         JOIN listing_sources s ON s.listing_source_id = r.listing_source_id \
         WHERE r.status = 'PENDING_REVIEW' AND s.crawl_enabled \
     ), \
     totals AS ( \
         SELECT COUNT(*) FILTER (WHERE agent_rounds < $4) AS workable, \
                COUNT(*) FILTER (WHERE agent_rounds >= $4) AS waiting_for_human \
         FROM queue \
     ), \
     page AS ( \
         SELECT * FROM queue \
         WHERE ($5::boolean IS NULL OR (agent_rounds >= $4) = $5) \
           AND ($6::timestamptz IS NULL OR (created, review_id) > ($6, $7::uuid)) \
         ORDER BY created, review_id \
         LIMIT $8 \
     ) \
     SELECT t.workable, t.waiting_for_human, p.* \
     FROM totals t LEFT JOIN page p ON TRUE \
     ORDER BY p.created, p.review_id"
);

/// The page of the agents' queue `query` asks for. A review that `query.after` names but that does
/// not exist is [`ReviewRepositoryError::NotFound`]: the reader starts again from the first page
/// instead of taking an empty page for the end of the queue.
pub(super) async fn fetch_pending_review_page(
    pool: &PgPool,
    agent_decisions: &AgentDecisionPolicy,
    query: PendingReviewQuery,
) -> Result<PendingReviewPage, ReviewRepositoryError> {
    let cursor = match query.after {
        Some(after) => Some(page_cursor(pool, after).await?),
        None => None,
    };
    let page_size = query.page_size.get();
    let (agents, repair_requested, human) = agent_round_binds();
    let rows = sqlx::query(PENDING_REVIEW_PAGE_QUERY)
        .bind(agents)
        .bind(repair_requested)
        .bind(human)
        .bind(i64::from(agent_decisions.max_repair_rounds.get()))
        .bind(query.human_required)
        .bind(cursor.map(|(created, _)| created))
        .bind(cursor.map(|(_, review_id)| *review_id.as_uuid()))
        .bind(i64::try_from(page_size.saturating_add(1)).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await?;

    let totals = match rows.first() {
        Some(row) => PendingReviewTotals {
            workable: row.try_get("workable")?,
            waiting_for_human: row.try_get("waiting_for_human")?,
        },
        None => return Err(sqlx::Error::RowNotFound.into()),
    };
    let mut reviews = Vec::with_capacity(rows.len());
    for row in &rows {
        if row.try_get::<Option<uuid::Uuid>, _>("review_id")?.is_some() {
            reviews.push(row_to_summary(row, agent_decisions)?);
        }
    }
    let next_after = if reviews.len() > page_size {
        reviews.truncate(page_size);
        reviews.last().map(|review| review.review_id)
    } else {
        None
    };
    Ok(PendingReviewPage {
        reviews,
        next_after,
        totals,
    })
}

/// The creation time and id of the review `after`, which a page starts after.
async fn page_cursor(
    pool: &PgPool,
    after: CrawlerReviewId,
) -> Result<(OffsetDateTime, CrawlerReviewId), ReviewRepositoryError> {
    sqlx::query_scalar::<_, OffsetDateTime>(
        "SELECT created FROM crawler_reviews WHERE review_id = $1",
    )
    .bind(after.as_uuid())
    .fetch_optional(pool)
    .await?
    .map(|created| (created, after))
    .ok_or(ReviewRepositoryError::NotFound(after))
}

fn row_to_summary(
    row: &PgRow,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<PendingReviewSummary, sqlx::Error> {
    // A count beyond `u32` is past any cap, so saturating keeps the review with a human.
    let agent_rounds = u32::try_from(row.try_get::<i64, _>("agent_rounds")?).unwrap_or(u32::MAX);
    Ok(PendingReviewSummary {
        review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
        listing_source_id: listing_source_id_from_uuid(row.try_get("listing_source_id")?)?,
        listing_source_name: row.try_get("listing_source_name")?,
        domain_id: row
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?,
        artifact_type: row.try_get("artifact_type")?,
        reason: row.try_get("reason")?,
        candidate_version: row.try_get("candidate_version")?,
        repair_request: row_to_repair_request(row)?,
        agent_rounds,
        human_required: agent_decisions.waits_for_human(STATUS_PENDING_REVIEW, agent_rounds),
        created: row.try_get("created")?,
        updated: row.try_get("updated")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_page_sizes_from_one_to_the_largest_page() {
        assert_eq!(PendingReviewPageSize::new(0), None);
        assert_eq!(
            PendingReviewPageSize::new(1).map(PendingReviewPageSize::get),
            Some(1)
        );
        assert_eq!(
            PendingReviewPageSize::new(500),
            Some(PendingReviewPageSize::MAX)
        );
        assert_eq!(PendingReviewPageSize::new(501), None);
        assert!(PendingReviewPageSize::DEFAULT.get() <= PendingReviewPageSize::MAX.get());
    }
}
