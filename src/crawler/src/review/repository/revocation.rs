//! A human's revocation of a rejected manual product-schema review: the rejection stops
//! classifying the review's seed and triggering pages as non-product pages, and those pages are
//! classified again by their ListingSource's URL-pattern decision, so they are scraped again and may open
//! a new review. The review stays rejected and keeps its history; the revocation joins its event
//! trail.

use super::rejected_pages::{RejectedPages, store_url_classes_clearing_retry};
use super::review_events::{LiveUrlPattern, ReviewMutation, commit_with_event};
use super::reviewer_page_url::ReviewPageIdentity;
use super::{
    CrawlerReviewRepository, ROLES_NO_REJECTION_JUDGES, ReviewRepositoryError,
    listing_source_id_from_uuid,
};
use crate::CrawlerReviewId;
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, ReviewActor, ReviewEventAction, STATUS_REJECTED,
};
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use crate::spider::classification::product_url_pattern::UrlPatternMatching;
use crate::spider::classification::url_classifier::UrlClassifier;
use crate::spider::classification::url_metadata::UrlClass;
use listing_source_core::ListingSourceId;
use sqlx::Row;

impl CrawlerReviewRepository {
    /// Revokes the rejection of a manual product-schema review, so it classifies none of its pages
    /// as non-product pages any more, and classifies its seed and triggering pages again by their
    /// ListingSource's live URL-pattern decision, as the spider classifies a URL it finds (a page another
    /// rejection covers stays `other`), without a retry delay: a page the pattern takes is scraped again when due and may open a new review. Only a
    /// human revokes, and only once; the review stays rejected.
    pub async fn revoke_rejection(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        if !actor.may_undo_decisions() {
            return Err(ReviewRepositoryError::RevocationNeedsHuman);
        }
        let mut transaction = self.pool.begin().await?;
        let listing_source_id = lock_revocable_rejection(&mut transaction, review_id).await?;
        let reclassified_urls = classify_rejected_pages_again(
            &mut transaction,
            review_id,
            &listing_source_id,
            self.url_classifier,
        )
        .await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::RejectionRevoked {
                notes,
                reclassified_urls,
            },
        )
        .await?;
        Ok(())
    }
}

/// Locks the rejected review, so its rejection is revoked once, and reads its ListingSource. Only
/// rejecting a manual product-schema review classifies pages, so only that rejection is revoked.
async fn lock_revocable_rejection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<ListingSourceId, ReviewRepositoryError> {
    let review = sqlx::query(
        "SELECT status, artifact_type, reason, listing_source_id \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    let classifies_pages = review.try_get::<String, _>("status")? == STATUS_REJECTED
        && review.try_get::<String, _>("artifact_type")? == ARTIFACT_PRODUCT_SCHEMA
        && review.try_get::<String, _>("reason")? == MANUAL_SCHEMA_REASON;
    if !classifies_pages {
        return Err(ReviewRepositoryError::NoRejectionToRevoke(review_id));
    }
    // Read after the row lock, so a revocation that waited for another one sees its event.
    let revoked = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM crawler_review_events \
         WHERE review_id = $1 AND action = $2)",
    )
    .bind(review_id.as_uuid())
    .bind(ReviewEventAction::RejectionRevoked.as_str())
    .fetch_one(&mut **transaction)
    .await?;
    if revoked {
        return Err(ReviewRepositoryError::RejectionAlreadyRevoked(review_id));
    }
    Ok(listing_source_id_from_uuid(
        review.try_get::<uuid::Uuid, _>("listing_source_id")?,
    )?)
}

/// Classifies the crawler URLs of the review's seed and triggering pages, in every spelling the
/// rejected-page lookup matches, again by their ListingSource's live URL-pattern decision with
/// `url_classifier`, or `other` where another unrevoked rejection covers the page, and clears
/// their retry delay. The rejection classified them `other` whether they were active or sold, so
/// both are classified again. The decision is read under a share lock until the revocation
/// commits, so an approval or restore of the source's URL pattern that commits meanwhile
/// classifies these URLs after it instead of being overwritten with the decision it replaced.
/// Returns the URLs whose class changed.
async fn classify_rejected_pages_again(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    url_classifier: UrlClassifier,
) -> Result<Vec<String>, ReviewRepositoryError> {
    let page_urls = sqlx::query_scalar::<_, String>(
        "SELECT url FROM crawler_review_pages WHERE review_id = $1 AND role <> ALL($2)",
    )
    .bind(review_id.as_uuid())
    .bind(ROLES_NO_REJECTION_JUDGES)
    .fetch_all(&mut **transaction)
    .await?;
    let spellings: Vec<String> = page_urls
        .iter()
        .flat_map(|url| ReviewPageIdentity::stored_spellings(url))
        .collect();
    let other_rejections =
        RejectedPages::load_without(&mut **transaction, listing_source_id, review_id).await?;
    let stored = sqlx::query(
        "SELECT u.url, u.url_class, s.url_pattern, s.url_pattern_exclude, s.url_pattern_matching, \
                s.url_pattern_state \
         FROM listing_source_urls u \
         JOIN listing_sources s ON s.listing_source_id = u.listing_source_id \
         WHERE u.listing_source_id = $1 AND u.url = ANY($2::text[]) \
           AND u.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
         FOR SHARE OF s",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(&spellings)
    .fetch_all(&mut **transaction)
    .await?;
    let mut urls = Vec::new();
    let mut classes = Vec::new();
    for row in stored {
        let url: String = row.try_get("url")?;
        let decision = LiveUrlPattern {
            url_pattern: row.try_get("url_pattern")?,
            url_pattern_exclude: row.try_get("url_pattern_exclude")?,
            url_pattern_matching: UrlPatternMatching::from_persisted_column(
                row.try_get::<Option<&str>, _>("url_pattern_matching")?,
            )
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            url_pattern_state: row.try_get("url_pattern_state")?,
        };
        let class = if other_rejections.covers(&url) {
            UrlClass::Other
        } else {
            url_classifier.classify(&url, decision.live_pattern()?.as_ref())
        }
        .to_string();
        if class != row.try_get::<String, _>("url_class")? {
            urls.push(url);
            classes.push(class);
        }
    }
    if urls.is_empty() {
        return Ok(urls);
    }
    store_url_classes_clearing_retry(transaction, listing_source_id, &urls, &classes).await?;
    Ok(urls)
}
