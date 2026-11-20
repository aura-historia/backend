//! The cost guard against the non-product page loop: a ListingSource whose URL pattern also takes
//! non-product pages opens a `manual_schema_required` review for each one it finds, and each ends
//! with the page ruled no product page. Once the ListingSource's latest such reviews closed that
//! way [`NON_PRODUCT_LOOP_CLOSES`] times in a row within [`NON_PRODUCT_LOOP_WINDOW_HOURS`], the
//! crawler hands the next one it opens to a human in the same transaction, so no agent works it.
//! The contract is in `docs/crawler/extraction-and-review.md` (non-product page loop).

use super::ROLES_NO_REJECTION_JUDGES;
use crate::review::model::{ReviewActor, ReviewEventAction, SystemHandoverReason};
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use listing_source_core::ListingSourceId;

/// How many of a ListingSource's latest closed `manual_schema_required` reviews in a row must
/// have ruled their opening pages no product pages before the next one goes to a human.
pub(super) const NON_PRODUCT_LOOP_CLOSES: usize = 2;

/// How recent, in hours, each of those closes must be. A source whose closes lie further apart
/// costs little, and an old close says little about its current URL pattern.
pub(super) const NON_PRODUCT_LOOP_WINDOW_HOURS: i32 = 24;

/// Whether the crawler event `e` of the review aliased `r` is a non-product loop hold that no
/// human answered since: a `SYSTEM` hand-over for [`SystemHandoverReason::NonProductPageLoop`] with no
/// `HUMAN` event of the review at or after it. Unlike an agent's hand-over it does not lapse with
/// the agents' round window, because a fresh agent attempt would only repeat the loop. Binds
/// nothing.
macro_rules! non_product_loop_hold_event {
    () => {
        "(e.actor = 'SYSTEM' AND e.action = 'HANDED_TO_HUMAN' \
          AND e.details->>'reason' = 'NON_PRODUCT_PAGE_LOOP' \
          AND NOT EXISTS (SELECT 1 FROM crawler_review_events h \
                          WHERE h.review_id = r.review_id AND h.actor = 'HUMAN' \
                            AND h.created >= e.created))"
    };
}
pub(crate) use non_product_loop_hold_event;

/// Whether the review aliased `r` is held for the non-product page loop
/// ([`non_product_loop_hold_event`]). Binds nothing.
macro_rules! held_for_non_product_loop {
    () => {
        concat!(
            "EXISTS (SELECT 1 FROM crawler_review_events e WHERE e.review_id = r.review_id AND ",
            $crate::review::repository::non_product_loop_hold_event!(),
            ")"
        )
    };
}
pub(crate) use held_for_non_product_loop;

/// One of a ListingSource's closed `manual_schema_required` reviews, newest first, as
/// [`NonProductLoop::detect`] judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClosedManualReview {
    /// Whether its verdict ruled its opening pages (its seed and triggering pages) no product
    /// pages: an approval with each of them marked as no product page, or a rejection no human
    /// revoked.
    pub ruled_no_product: bool,
    /// Whether it closed within [`NON_PRODUCT_LOOP_WINDOW_HOURS`].
    pub within_window: bool,
    /// The URLs of its opening pages.
    pub opening_urls: Vec<String>,
}

/// A ListingSource's latest `manual_schema_required` reviews that closed with their opening pages
/// ruled no product pages, in a row and recently enough to hold its next one for a human. A
/// rejection that rests on the page classifier's verdicts neither counts nor breaks the row: the
/// classifier spends no agent rounds, so the loop it guards against does not arise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NonProductLoop {
    /// The opening pages of those reviews, newest first.
    ruled_urls: Vec<String>,
}

impl NonProductLoop {
    /// The loop `latest`, the ListingSource's latest closed `manual_schema_required` reviews newest
    /// first, shows, or `None` when fewer than [`NON_PRODUCT_LOOP_CLOSES`] of them closed or any of
    /// that many latest ones closed otherwise or too long ago.
    pub(super) fn of(latest: &[ClosedManualReview]) -> Option<Self> {
        let latest = latest.get(..NON_PRODUCT_LOOP_CLOSES)?;
        latest
            .iter()
            .all(|review| review.ruled_no_product && review.within_window)
            .then(|| Self {
                ruled_urls: latest
                    .iter()
                    .flat_map(|review| review.opening_urls.iter().cloned())
                    .collect(),
            })
    }

    /// Reads the loop of `listing_source_id`'s latest closed `manual_schema_required` reviews.
    pub(super) async fn detect<'e>(
        executor: impl sqlx::PgExecutor<'e>,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<Self>, sqlx::Error> {
        let latest = sqlx::query_as::<_, (bool, bool, Vec<String>)>(concat!(
            "SELECT \
                 CASE WHEN r.status = 'REJECTED' THEN NOT EXISTS ( \
                          SELECT 1 FROM crawler_review_events e \
                          WHERE e.review_id = r.review_id AND e.action = $3) \
                      ELSE EXISTS (SELECT 1 FROM crawler_review_pages p \
                                   WHERE p.review_id = r.review_id AND p.role <> ALL($4)) \
                           AND NOT EXISTS (SELECT 1 FROM crawler_review_pages p \
                                           WHERE p.review_id = r.review_id AND p.role <> ALL($4) \
                                             AND p.not_product_reason IS NULL) \
                 END AS ruled_no_product, \
                 r.reviewed > NOW() - make_interval(hours => $5) AS within_window, \
                 ARRAY(SELECT p.url FROM crawler_review_pages p \
                       WHERE p.review_id = r.review_id AND p.role <> ALL($4) \
                       ORDER BY p.url) AS opening_urls \
             FROM crawler_reviews r \
             WHERE r.listing_source_id = $1 AND r.artifact_type = 'PRODUCT_SCHEMA' \
               AND r.reason = $2 AND r.status IN ('APPROVED', 'REJECTED') \
               AND r.reviewed IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM crawler_review_events e \
                               WHERE e.review_id = r.review_id AND ",
            crate::review::repository::classifier_rejection_event!(),
            ") \
             ORDER BY r.reviewed DESC, r.review_id DESC \
             LIMIT $6"
        ))
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(MANUAL_SCHEMA_REASON)
        .bind(ReviewEventAction::RejectionRevoked.as_str())
        .bind(ROLES_NO_REJECTION_JUDGES)
        .bind(NON_PRODUCT_LOOP_WINDOW_HOURS)
        .bind(NON_PRODUCT_LOOP_CLOSES as i64)
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(
            |(ruled_no_product, within_window, opening_urls)| ClosedManualReview {
                ruled_no_product,
                within_window,
                opening_urls,
            },
        )
        .collect::<Vec<_>>();
        Ok(Self::of(&latest))
    }

    /// The cause the hand-over records.
    pub(super) const REASON: SystemHandoverReason = SystemHandoverReason::NonProductPageLoop;

    /// The actor that records the hand-over: the crawler itself.
    pub(super) const ACTOR: ReviewActor = ReviewActor::System;

    /// The hand-over's note for the human: what the crawler saw and that the URL pattern is the
    /// likely cause.
    pub(super) fn note(&self) -> String {
        format!(
            "The last {NON_PRODUCT_LOOP_CLOSES} product-schema reviews of this source closed within \
             {NON_PRODUCT_LOOP_WINDOW_HOURS} hours with their pages ruled no product pages ({}). \
             The live URL pattern likely also takes non-product pages, so an agent review of this \
             page would end the same way. Narrow the URL pattern or label the source, then decide \
             this review.",
            self.ruled_urls.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(ruled_no_product: bool, within_window: bool, url: &str) -> ClosedManualReview {
        ClosedManualReview {
            ruled_no_product,
            within_window,
            opening_urls: vec![url.to_owned()],
        }
    }

    #[test]
    fn should_hold_after_two_recent_no_product_closes_in_a_row() {
        let latest = [
            closed(true, true, "https://shop.example/store/collections/b"),
            closed(true, true, "https://shop.example/store/collections/a"),
        ];

        let found = NonProductLoop::of(&latest).expect("loop");

        assert_eq!(
            found.ruled_urls,
            [
                "https://shop.example/store/collections/b",
                "https://shop.example/store/collections/a"
            ]
        );
        let note = found.note();
        assert!(note.contains("URL pattern"), "{note}");
        assert!(
            note.contains("https://shop.example/store/collections/a"),
            "{note}"
        );
    }

    #[test]
    fn should_not_hold_after_a_single_no_product_close() {
        assert_eq!(NonProductLoop::of(&[closed(true, true, "u")]), None);
        assert_eq!(NonProductLoop::of(&[]), None);
    }

    #[rstest::rstest]
    #[case::latest_extracted_a_product([closed(false, true, "b"), closed(true, true, "a")])]
    #[case::older_extracted_a_product([closed(true, true, "b"), closed(false, true, "a")])]
    #[case::older_closed_too_long_ago([closed(true, true, "b"), closed(true, false, "a")])]
    fn should_not_hold_when_the_latest_closes_break_the_streak(
        #[case] latest: [ClosedManualReview; 2],
    ) {
        assert_eq!(NonProductLoop::of(&latest), None);
    }

    #[test]
    fn should_judge_only_the_latest_closes() {
        let latest = [
            closed(true, true, "c"),
            closed(true, true, "b"),
            closed(false, false, "a"),
        ];

        assert!(NonProductLoop::of(&latest).is_some());
    }

    #[test]
    fn should_name_the_persisted_values_the_hold_condition_matches() {
        let condition = held_for_non_product_loop!();
        for persisted in [
            NonProductLoop::ACTOR.as_str(),
            ReviewEventAction::HandedToHuman.as_str(),
            ReviewActor::Human.as_str(),
        ] {
            assert!(condition.contains(&format!("'{persisted}'")), "{persisted}");
        }
        assert_eq!(
            serde_json::to_value(NonProductLoop::REASON).ok(),
            Some(serde_json::Value::from("NON_PRODUCT_PAGE_LOOP"))
        );
        assert!(condition.contains("'NON_PRODUCT_PAGE_LOOP'"));
    }
}
