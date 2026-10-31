//! A human's revocation of a rejected manual product-schema review: the rejection stops
//! classifying the review's seed and triggering pages as non-product pages, and those pages are
//! classified again by their domain's URL-pattern decision, so they are scraped again and may open
//! a new review. The review stays rejected and keeps its history; the revocation joins its event
//! trail.

use super::review_events::{DomainUrlPattern, ReviewMutation, commit_with_event};
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
use crate::spider::classification::url_classifier::UrlClassifier;
use listing_source_core::ListingSourceId;
use sqlx::Row;
use tracing::info;

impl CrawlerReviewRepository {
    /// Revokes the rejection of a manual product-schema review, so it classifies none of its pages
    /// as non-product pages any more, and classifies its seed and triggering pages again by their
    /// domain's live URL-pattern decision, as the spider classifies a URL it finds, without a retry
    /// delay: a page the pattern takes is scraped again when due and may open a new review. Only a
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
        let reclassified_count = reclassified_urls.len();
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
        info!(
            review_id = %review_id,
            actor = %actor,
            reclassified_urls = reclassified_count,
            "Revoked the rejection of a manual product schema review"
        );
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
/// rejected-page lookup matches, again by their domain's live URL-pattern decision with `url_classifier` and clears
/// their retry delay. The rejection classified them `other` whether they were active or sold, so
/// both are classified again. The decision is read under a share lock until the revocation
/// commits, so an approval or restore of the domain's URL pattern that commits meanwhile
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
    let stored = sqlx::query(
        "SELECT u.url, u.url_class, d.url_pattern, d.url_pattern_state \
         FROM listing_source_urls u \
         JOIN listing_source_domains d \
           ON d.listing_source_id = u.listing_source_id AND d.domain_id = u.domain_id \
         WHERE u.listing_source_id = $1 AND u.url = ANY($2::text[]) \
           AND u.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
         FOR SHARE OF d",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(&spellings)
    .fetch_all(&mut **transaction)
    .await?;
    let mut urls = Vec::new();
    let mut classes = Vec::new();
    for row in stored {
        let url: String = row.try_get("url")?;
        let decision = DomainUrlPattern {
            url_pattern: row.try_get("url_pattern")?,
            url_pattern_state: row.try_get("url_pattern_state")?,
        };
        let class = url_classifier
            .classify(&url, decision.live_pattern()?.as_ref())
            .to_string();
        if class != row.try_get::<String, _>("url_class")? {
            urls.push(url);
            classes.push(class);
        }
    }
    if urls.is_empty() {
        return Ok(urls);
    }
    sqlx::query(
        "UPDATE listing_source_urls AS u \
         SET url_class = input.url_class, next_retry_at = NULL, updated = NOW() \
         FROM UNNEST($1::text[], $2::text[]) AS input(url, url_class) \
         WHERE u.url = input.url AND u.listing_source_id = $3",
    )
    .bind(&urls)
    .bind(&classes)
    .bind(uuid::Uuid::from(*listing_source_id))
    .execute(&mut **transaction)
    .await?;
    Ok(urls)
}
