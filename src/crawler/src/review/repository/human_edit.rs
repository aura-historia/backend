//! The reviews a human opens from a ListingSource's page to edit its live URL pattern or product
//! schemas: a pending review whose candidate is the live configuration, which the human edits in
//! the review workbench and approves through the review rail like any other, never a direct write
//! of the live configuration. The crawler hands the review to humans in the transaction that opens
//! it, so the review agents never take it over. The contract is in
//! `docs/crawler/extraction-and-review.md` (human edits of the live configuration).

use super::review_events::{ReviewMutation, commit_with_event};
use super::review_log::{ReviewLogEntry, log_committed_review};
use super::url_pattern_sample::{MAX_SAMPLE_URLS, candidate_product_count, insert_sample_urls};
use super::verdicts::read_live_url_pattern;
use super::{
    CrawlerReviewRepository, NewSchemaReviewRow, OpeningCandidate, ReviewPlacement,
    ReviewRepositoryError, insert_review_page, place_schema_review, place_url_pattern_review,
};
use crate::CrawlerReviewId;
use crate::review::model::{
    ArtifactType, PAGE_ROLE_SEED, ReviewActor, STATUS_PENDING_REVIEW, SystemHandoverReason,
    UrlPatternDecision, UrlPatternReviewCandidate,
};
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::classification::url_pattern_repository::UrlPatternState;
use listing_source_core::ListingSourceId;
use serde_json::json;
use url::Url;

/// The reason of a review a human opened to edit the live configuration, of either artifact type.
pub const HUMAN_EDIT_REASON: &str = "human_edit_requested";

/// How many recently captured product pages a product-schema edit opens with, as many as the live
/// schema audit samples.
pub const HUMAN_EDIT_PAGES: i64 = 5;

/// Whether the event `e` of the review aliased `r` is the crawler's hand-over of a review a human
/// opened to edit: a `SYSTEM` hand-over for [`SystemHandoverReason::HumanEdit`]. Unlike every other
/// hand-over no human change lifts it, since the human who opened the review edits it, and it
/// does not lapse: the review stays with humans until it is decided. Binds nothing.
macro_rules! human_edit_hold_event {
    () => {
        "(e.actor = 'SYSTEM' AND e.action = 'HANDED_TO_HUMAN' \
          AND e.details->>'reason' = 'HUMAN_EDIT')"
    };
}
pub(crate) use human_edit_hold_event;

/// Whether the review aliased `r` is held for the humans who edit it
/// ([`human_edit_hold_event`]). Such a review waits for a human, but as its own work rather than as
/// one the review agents left, so the agents' queue and the counts of reviews that unexpectedly
/// wait for a human leave it out. Binds nothing.
macro_rules! held_for_human_edit {
    () => {
        concat!(
            "EXISTS (SELECT 1 FROM crawler_review_events e WHERE e.review_id = r.review_id AND ",
            $crate::review::repository::human_edit_hold_event!(),
            ")"
        )
    };
}
pub(crate) use held_for_human_edit;

impl CrawlerReviewRepository {
    /// Opens the review in which a human edits the ListingSource's live `artifact`: a pending
    /// review of reason [`HUMAN_EDIT_REASON`] whose candidate is the live configuration, handed to
    /// humans in the transaction that opens it. When a review of that artifact is already pending,
    /// the source's one, it is answered as it is instead, so a second review never opens. Only a
    /// human opens one. A ListingSource that has no live product schemas, or for a URL pattern no
    /// crawl root, is refused, as is one whose live configuration does not read as one.
    pub async fn open_human_edit_review(
        &self,
        listing_source_id: &ListingSourceId,
        artifact: ArtifactType,
        actor: ReviewActor,
    ) -> Result<ReviewPlacement, ReviewRepositoryError> {
        if actor != ReviewActor::Human {
            return Err(ReviewRepositoryError::HumanEditNeedsHuman);
        }
        if let Some(pending) = self
            .latest_pending_review_id(listing_source_id, artifact.as_str())
            .await?
        {
            return Ok(ReviewPlacement::Joined(pending));
        }
        match artifact {
            ArtifactType::ProductSchema => self.open_product_schema_edit(listing_source_id).await,
            ArtifactType::UrlPattern => self.open_url_pattern_edit(listing_source_id).await,
        }
    }

    /// Opens a product-schema review of the live schemas and stored removed-page templates over
    /// the source's most recently captured product pages. The pages are stored without a fetch;
    /// the human's live matrix refresh fetches them, as it fetches every review page.
    async fn open_product_schema_edit(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<ReviewPlacement, ReviewRepositoryError> {
        self.require_listing_source(listing_source_id).await?;
        let live_schemas = ListingSourceProductSchemaRepositoryImpl::new(&self.pool)
            .find_product_schema(listing_source_id)
            .await?
            .map(|live| live.product_schemas)
            .unwrap_or_default();
        if live_schemas.is_empty() {
            return Err(ReviewRepositoryError::HumanEditNotPossible(
                *listing_source_id,
                "it has no live product schemas; its first product-schema review opens when the scraper meets one of its product pages",
            ));
        }
        let OpeningCandidate {
            candidate_payload,
            removed_page_schemas_base,
        } = self
            .opening_candidate(listing_source_id, &live_schemas)
            .await
            .map_err(|error| match error {
                ReviewRepositoryError::InvalidProductSchemaCandidate => {
                    ReviewRepositoryError::HumanEditNotPossible(
                        *listing_source_id,
                        "its live product schemas fail today's validation; repair them on the approved review whose candidate is live",
                    )
                }
                error => error,
            })?;
        let pages = self
            .recently_captured_product_urls(listing_source_id)
            .await?;
        let mut transaction = self.pool.begin().await?;
        let placement = place_schema_review(
            &mut transaction,
            NewSchemaReviewRow {
                listing_source_id,
                reason: HUMAN_EDIT_REASON,
                candidate_payload,
                validation_summary: json!({}),
                status: STATUS_PENDING_REVIEW,
                notes: None,
                removed_page_schemas_base,
            },
        )
        .await?;
        let ReviewPlacement::Opened(review_id) = placement else {
            transaction.commit().await?;
            return Ok(placement);
        };
        for url in &pages {
            // Nothing was fetched, so the page records the hash of no content.
            insert_review_page(&mut transaction, review_id, url, PAGE_ROLE_SEED, "").await?;
        }
        self.commit_held_for_human_edit(transaction, review_id, ArtifactType::ProductSchema)
            .await?;
        Ok(placement)
    }

    /// Opens a URL-pattern review of the live decision, `UNDECIDED` while the source has none,
    /// with a sample of the source's stored URLs classified under it, so the workbench shows which
    /// the pattern takes. A source without a crawl root is refused: no URL of it can be checked and
    /// no decision of it approved.
    async fn open_url_pattern_edit(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<ReviewPlacement, ReviewRepositoryError> {
        let Some(live) = read_live_url_pattern(
            &self.pool,
            listing_source_id,
            "SELECT url_pattern, url_pattern_exclude, url_pattern_matching, url_pattern_state \
             FROM listing_sources WHERE listing_source_id = $1",
        )
        .await?
        else {
            return Err(ReviewRepositoryError::ListingSourceNotFound(
                *listing_source_id,
            ));
        };
        if self
            .listing_source_crawl_root(listing_source_id)
            .await?
            .is_none()
        {
            return Err(ReviewRepositoryError::HumanEditNotPossible(
                *listing_source_id,
                "it has no crawl root, so no URL of it can be classified and no URL pattern of it approved",
            ));
        }
        let current_pattern = live.live_pattern()?;
        let decision = live_decision(&live.url_pattern_state, current_pattern.as_ref())?;
        let sample = self.stored_url_sample(listing_source_id).await?;
        let sample_urls: Vec<String> = sample.iter().map(|(url, _)| url.clone()).collect();
        let validation_summary = json!({
            "sample_count": sample.len(),
            "candidate_product_count": candidate_product_count(&sample_urls, current_pattern.as_ref()),
        });
        let candidate_payload = serde_json::to_value(UrlPatternReviewCandidate::new(
            decision,
            current_pattern.as_ref(),
        ))?;
        let mut transaction = self.pool.begin().await?;
        let placement = place_url_pattern_review(
            &mut transaction,
            listing_source_id,
            HUMAN_EDIT_REASON,
            candidate_payload,
            validation_summary,
        )
        .await?;
        let ReviewPlacement::Opened(review_id) = placement else {
            transaction.commit().await?;
            return Ok(placement);
        };
        insert_sample_urls(
            &mut transaction,
            review_id,
            &sample,
            self.url_classifier,
            current_pattern.as_ref(),
            current_pattern.as_ref(),
        )
        .await?;
        self.commit_held_for_human_edit(transaction, review_id, ArtifactType::UrlPattern)
            .await?;
        Ok(placement)
    }

    /// Hands the review a human opened to edit `artifact` to humans as the crawler's
    /// [`SystemHandoverReason::HumanEdit`] hand-over and commits the transaction that opened it.
    async fn commit_held_for_human_edit(
        &self,
        transaction: sqlx::Transaction<'_, sqlx::Postgres>,
        review_id: CrawlerReviewId,
        artifact: ArtifactType,
    ) -> Result<(), ReviewRepositoryError> {
        let edited = match artifact {
            ArtifactType::ProductSchema => "product schemas",
            ArtifactType::UrlPattern => "URL pattern",
        };
        commit_with_event(
            transaction,
            review_id,
            ReviewActor::System,
            ReviewMutation::HandedToHumanByCrawler {
                note: &format!(
                    "A human opened this review from the source page to edit the live {edited}; the review agents leave it to humans until it is approved or rejected."
                ),
                reason: SystemHandoverReason::HumanEdit,
            },
        )
        .await?;
        log_committed_review(
            &self.pool,
            review_id,
            vec![ReviewLogEntry::opened(HUMAN_EDIT_REASON)],
        )
        .await;
        Ok(())
    }

    async fn require_listing_source(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), ReviewRepositoryError> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&self.pool)
        .await?;
        if exists {
            Ok(())
        } else {
            Err(ReviewRepositoryError::ListingSourceNotFound(
                *listing_source_id,
            ))
        }
    }

    /// At most [`HUMAN_EDIT_PAGES`] active or sold product URLs of the ListingSource whose listings
    /// were captured, those that last scraped without an error first, the most recently scraped
    /// first. A stored URL that no longer parses is left out.
    async fn recently_captured_product_urls(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<String>, ReviewRepositoryError> {
        let urls: Vec<String> = sqlx::query_scalar(
            "SELECT url FROM listing_source_urls
             WHERE listing_source_id = $1
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_source_listing_id IS NOT NULL
             ORDER BY last_error_kind IS NULL DESC, last_scraped DESC NULLS LAST, url
             LIMIT $2",
        )
        .bind(listing_source_id.as_uuid())
        .bind(HUMAN_EDIT_PAGES)
        .fetch_all(&self.pool)
        .await?;
        Ok(urls
            .into_iter()
            .filter(|url| Url::parse(url).is_ok())
            .collect())
    }

    /// Up to [`MAX_SAMPLE_URLS`] of the ListingSource's stored URLs, each with its stored class,
    /// picked in the order of a hash of the URL so URLs of every path shape are in reach, as a
    /// sample refresh picks them.
    async fn stored_url_sample(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<(String, String)>, ReviewRepositoryError> {
        Ok(sqlx::query_as(
            "SELECT url, url_class FROM listing_source_urls
             WHERE listing_source_id = $1
             ORDER BY md5(url), url
             LIMIT $2",
        )
        .bind(listing_source_id.as_uuid())
        .bind(MAX_SAMPLE_URLS)
        .fetch_all(&self.pool)
        .await?)
    }
}

/// The decision a URL-pattern edit opens with for the live `state` and its `pattern`: the pattern
/// of a `MATCHED` source, `NO_PATTERN`, or `UNDECIDED` for a source never decided. A state no
/// decision has, or a `MATCHED` one without a pattern, fails closed.
fn live_decision(
    state: &str,
    pattern: Option<&ProductUrlPattern>,
) -> Result<UrlPatternDecision, sqlx::Error> {
    let invalid = |problem: String| sqlx::Error::Decode(problem.into());
    if state == UrlPatternState::Matched.as_str() {
        pattern
            .map(UrlPatternDecision::of_pattern)
            .ok_or_else(|| invalid("a MATCHED URL-pattern decision without a pattern".to_owned()))
    } else if state == UrlPatternState::NoPattern.as_str() {
        Ok(UrlPatternDecision::NoPattern)
    } else if state == UrlPatternState::Unknown.as_str() {
        Ok(UrlPatternDecision::Undecided)
    } else {
        Err(invalid(format!(
            "invalid persisted URL-pattern state `{state}`"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::classification::product_url_pattern::UrlPatternMatching;

    #[test]
    fn should_open_with_the_live_decision_of_each_state() {
        let pattern =
            ProductUrlPattern::parse_matching("/item/", UrlPatternMatching::FullUrlSearch)
                .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            live_decision("MATCHED", Some(&pattern)).ok(),
            Some(UrlPatternDecision::of_pattern(&pattern))
        );
        assert_eq!(
            live_decision("NO_PATTERN", None).ok(),
            Some(UrlPatternDecision::NoPattern)
        );
        assert_eq!(
            live_decision("UNKNOWN", None).ok(),
            Some(UrlPatternDecision::Undecided)
        );
    }

    #[test]
    fn should_fail_closed_on_a_live_decision_no_state_reads_as() {
        assert!(live_decision("MATCHED", None).is_err());
        assert!(live_decision("matched", None).is_err());
        assert!(live_decision("", None).is_err());
    }
}
