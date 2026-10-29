//! A human's restore of what an approval replaced: the ListingSource's schema list and
//! removed-page templates, or the domain's URL-pattern decision, as the approval's event keeps
//! them. Any approval whose event keeps what it made live can be restored, whoever approved it,
//! once and only while that is still the live configuration.

use super::review_events::{
    ProductSchemaApprovalRecord, RecordedApproval, ReviewMutation, UrlPatternApprovalRecord,
    commit_with_event, fetch_recorded_approval,
};
use super::verdicts::{
    WaitingWork, lock_domain_url_pattern, make_url_pattern_live, release_work_waiting_on,
};
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, crawler_domain_id_from_uuid,
    listing_source_id_from_uuid, lock_live_product_schemas, persist_product_schemas,
};
use crate::CrawlerReviewId;
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, ARTIFACT_URL_PATTERN, ReviewActor, STATUS_APPROVED,
};
use crate::scraper::css_selector::removed_page_schema_repository::{
    lock_stored_removed_page_schemas, store_removed_page_schemas,
};
use crate::spider::classification::url_classifier::UrlClassifier;
use listing_source_core::ListingSourceId;
use serde::de::DeserializeOwned;
use sqlx::Row;
use tracing::info;

impl CrawlerReviewRepository {
    /// Brings back the live configuration the review's approval replaced and releases the work
    /// waiting on it, as the approval did. Only a human restores, and only while the configuration
    /// the approval made live is still live: a newer change is never silently overwritten.
    pub async fn restore_approval(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        if !actor.may_undo_decisions() {
            return Err(ReviewRepositoryError::RestoreNeedsHuman);
        }
        let mut transaction = self.pool.begin().await?;
        let review = lock_approved_review(&mut transaction, review_id).await?;
        let approval = fetch_recorded_approval(&mut *transaction, review_id)
            .await?
            .ok_or(ReviewRepositoryError::NoApprovalToRestore(review_id))?;
        if approval.restored {
            return Err(ReviewRepositoryError::ApprovalAlreadyRestored(review_id));
        }
        let restore = match review.artifact_type.as_str() {
            ARTIFACT_PRODUCT_SCHEMA => {
                restore_product_schemas(
                    &mut transaction,
                    review_id,
                    &review.listing_source_id,
                    &approval,
                    notes,
                )
                .await?
            }
            ARTIFACT_URL_PATTERN => {
                restore_url_pattern(
                    &mut transaction,
                    review_id,
                    &review,
                    &approval,
                    notes,
                    self.url_classifier,
                )
                .await?
            }
            other => {
                return Err(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    other.into(),
                ));
            }
        };
        release_work_waiting_on(&mut transaction, &review).await?;
        commit_with_event(transaction, review_id, actor, restore).await?;
        info!(review_id = %review_id, actor = %actor, "Restored what a review's approval replaced");
        Ok(())
    }
}

/// Locks the review whose approval is restored, so it restores once, and reads what the restore
/// re-queues. Only an approved review has an approval to restore.
async fn lock_approved_review(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<WaitingWork, ReviewRepositoryError> {
    let review = sqlx::query(
        "SELECT status, listing_source_id, domain_id, artifact_type \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    if review.try_get::<String, _>("status")? != STATUS_APPROVED {
        return Err(ReviewRepositoryError::NoApprovalToRestore(review_id));
    }
    Ok(WaitingWork {
        artifact_type: review.try_get("artifact_type")?,
        listing_source_id: listing_source_id_from_uuid(
            review.try_get::<uuid::Uuid, _>("listing_source_id")?,
        )?,
        domain_id: review
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?,
    })
}

/// What the approval's event keeps, refused when it predates restorable approvals.
fn approval_record<T: DeserializeOwned>(
    review_id: CrawlerReviewId,
    approval: &RecordedApproval,
) -> Result<T, ReviewRepositoryError> {
    serde_json::from_value(approval.details.clone())
        .map_err(|source| ReviewRepositoryError::ApprovalNotRestorable { review_id, source })
}

/// Stores the schema list and removed-page templates the approval replaced, under the same locks
/// approval takes, while the ones it made live are still live. Restoring an approval that
/// replaced no schema list leaves the ListingSource without one.
async fn restore_product_schemas<'a>(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    approval: &RecordedApproval,
    notes: Option<&'a str>,
) -> Result<ReviewMutation<'a>, ReviewRepositoryError> {
    let record: ProductSchemaApprovalRecord = approval_record(review_id, approval)?;
    let live = lock_live_product_schemas(transaction, listing_source_id).await?;
    let stored = lock_stored_removed_page_schemas(transaction, listing_source_id)
        .await?
        .unwrap_or_default();
    if live.as_ref().map(|live| &live.product_schemas) != Some(&record.approved_product_schemas)
        || stored != record.approved_removed_page_schemas
    {
        return Err(ReviewRepositoryError::LiveConfigurationChangedSinceApproval(review_id));
    }
    match &record.replaced_product_schemas {
        Some(schemas) => persist_product_schemas(transaction, listing_source_id, schemas).await?,
        None => {
            sqlx::query("DELETE FROM listing_source_product_schemas WHERE listing_source_id = $1")
                .bind(uuid::Uuid::from(*listing_source_id))
                .execute(&mut **transaction)
                .await?;
        }
    }
    if record.replaced_removed_page_schemas != stored {
        store_removed_page_schemas(
            transaction,
            listing_source_id,
            &record.replaced_removed_page_schemas,
        )
        .await?;
    }
    info!(
        review_id = %review_id,
        listing_source_id = %listing_source_id,
        schema_count = record.replaced_product_schemas.as_ref().map_or(0, Vec::len),
        removed_page_template_count = record.replaced_removed_page_schemas.len(),
        "Restored the product schemas an approval replaced"
    );
    Ok(ReviewMutation::ProductSchemasRestored {
        notes,
        restored_event_id: approval.event_id,
        replaced_product_schemas: record.approved_product_schemas,
        replaced_removed_page_schemas: record.approved_removed_page_schemas,
    })
}

/// Stores the URL-pattern decision the approval replaced, while the one it made live is still
/// live, and classifies the domain's stored URLs with it by `url_classifier` as approval does.
async fn restore_url_pattern<'a>(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    review: &WaitingWork,
    approval: &RecordedApproval,
    notes: Option<&'a str>,
    url_classifier: UrlClassifier,
) -> Result<ReviewMutation<'a>, ReviewRepositoryError> {
    let record: UrlPatternApprovalRecord = approval_record(review_id, approval)?;
    let domain_id = review.domain_id.ok_or(sqlx::Error::RowNotFound)?;
    let live = lock_domain_url_pattern(transaction, &review.listing_source_id, domain_id)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    if live != record.approved_url_pattern {
        return Err(ReviewRepositoryError::LiveConfigurationChangedSinceApproval(review_id));
    }
    let reclassified = make_url_pattern_live(
        transaction,
        &review.listing_source_id,
        domain_id,
        &record.replaced_url_pattern,
        url_classifier,
    )
    .await?;
    info!(
        review_id = %review_id,
        domain_id = %domain_id,
        reclassified_urls = reclassified,
        "Restored the URL pattern an approval replaced"
    );
    Ok(ReviewMutation::UrlPatternRestored {
        notes,
        restored_event_id: approval.event_id,
        replaced_url_pattern: record.approved_url_pattern,
    })
}
