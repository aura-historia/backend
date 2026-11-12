//! The human queue as CloudWatch metrics: how many pending reviews wait for a human and how long
//! ago the oldest of them opened, so the backlog the review agents leave to humans shows without
//! the console. Only `agent` mode has such a queue, so only its console reports it. The metric
//! names are in `docs/crawler/operations.md` (metrics and alarms).

use crate::metrics::{CrawlerMetric, MetricRecord};
use crate::review::model::{PageSize, PendingReviewPage};
use crate::review::repository::{CrawlerReviewRepository, PendingReviewQuery};
use std::convert::Infallible;
use std::time::Duration;
use time::OffsetDateTime;
use tracing::warn;

/// How often the console reports the human queue.
const REPORT_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Reports the human queue of `repository` every [`REPORT_INTERVAL`] while its review agents
/// work, from the first tick on; outside `agent` mode it waits forever and reports nothing. A
/// queue that cannot be read is warned about and reported at the next tick.
pub(super) async fn report_while_agents_work(repository: CrawlerReviewRepository) -> Infallible {
    if repository.agent_decisions().repair_rounds_cap().is_none() {
        return std::future::pending().await;
    }
    let mut ticks = tokio::time::interval(REPORT_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        match repository
            .list_pending_reviews(oldest_waiting_query())
            .await
        {
            Ok(page) => human_queue_record(&page, OffsetDateTime::now_utc()).emit(),
            Err(error) => warn!(
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Cannot read the reviews that wait for a human; the next report reads them again"
            ),
        }
    }
}

/// The first page of the reviews that wait for a human, oldest first, which carries the count of
/// all of them and the oldest one.
fn oldest_waiting_query() -> PendingReviewQuery {
    PendingReviewQuery {
        human_required: Some(true),
        after: None,
        page_size: PageSize::new(1).unwrap_or(PageSize::DEFAULT),
    }
}

/// The metrics of `page`, the first page of [`oldest_waiting_query`], at `now`. The age is 0 when
/// no review waits, so the metric has a value at every report.
fn human_queue_record(page: &PendingReviewPage, now: OffsetDateTime) -> MetricRecord {
    let waiting = u64::try_from(page.totals.waiting_for_human).unwrap_or(0);
    let oldest_age_seconds = page
        .reviews
        .first()
        .map(|oldest| u64::try_from((now - oldest.created).whole_seconds()).unwrap_or(0))
        .unwrap_or(0);
    MetricRecord::global()
        .with_value(CrawlerMetric::ReviewsWaitingForHuman, waiting)
        .with_value(
            CrawlerMetric::OldestReviewWaitingForHumanAge,
            oldest_age_seconds,
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrawlerReviewId;
    use crate::review::model::{PendingReviewSummary, PendingReviewTotals};
    use listing_source_core::ListingSourceId;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-11 18:00 UTC);

    fn page(waiting_for_human: i64, oldest: Option<OffsetDateTime>) -> PendingReviewPage {
        PendingReviewPage {
            reviews: oldest
                .map(|created| PendingReviewSummary {
                    review_id: CrawlerReviewId::new(),
                    listing_source_id: ListingSourceId::new(),
                    listing_source_name: "Antiques".to_owned(),
                    artifact_type: "URL_PATTERN".to_owned(),
                    reason: "manual_url_pattern_required".to_owned(),
                    candidate_version: 2,
                    repair_request: None,
                    agent_rounds: 0,
                    human_required: true,
                    handed_back: None,
                    created,
                    updated: created,
                })
                .into_iter()
                .collect(),
            next_after: None,
            totals: PendingReviewTotals {
                workable: 7,
                waiting_for_human,
            },
        }
    }

    #[test]
    fn should_report_how_many_reviews_wait_for_a_human_and_the_age_of_the_oldest() {
        let record = human_queue_record(&page(182, Some(NOW - time::Duration::hours(26))), NOW);

        assert_eq!(
            record,
            MetricRecord::global()
                .with_value(CrawlerMetric::ReviewsWaitingForHuman, 182)
                .with_value(CrawlerMetric::OldestReviewWaitingForHumanAge, 26 * 3600)
        );
    }

    #[test]
    fn should_report_zero_when_no_review_waits_for_a_human() {
        assert_eq!(
            human_queue_record(&page(0, None), NOW),
            MetricRecord::global()
                .with_value(CrawlerMetric::ReviewsWaitingForHuman, 0)
                .with_value(CrawlerMetric::OldestReviewWaitingForHumanAge, 0)
        );
    }

    #[test]
    fn should_read_only_the_oldest_review_that_waits_for_a_human() {
        let query = oldest_waiting_query();

        assert_eq!(
            (query.human_required, query.after, query.page_size.get()),
            (Some(true), None, 1)
        );
    }
}
