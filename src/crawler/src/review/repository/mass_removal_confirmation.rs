//! An operator's confirmation that a ListingSource's removals are a real bulk clearance, which
//! the scraper's mass-removal guard honours until it ends, and its revocation
//! (`docs/crawler/scheduling-and-retries.md`, mass-removal guard).

use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::model::ReviewActor;
use crate::scraper::removal_guard::{MASS_REMOVAL_HELD_ERROR_KIND, MassRemovalConfirmationHours};
use listing_source_core::ListingSourceId;
use serde::Serialize;
use time::OffsetDateTime;
use tracing::info;

/// A stored confirmation as `POST /api/listing-sources/{id}/mass-removal-confirmation` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MassRemovalConfirmation {
    #[serde(with = "time::serde::rfc3339")]
    pub mass_removal_confirmed_until: OffsetDateTime,
    pub mass_removal_confirmed_by: ReviewActor,
    /// The source's `MassRemovalHeld` URLs the confirmation made due at once.
    pub rearmed_held_urls: u64,
}

impl CrawlerReviewRepository {
    /// Confirms the ListingSource's mass removal for `hours` from now, replacing any earlier
    /// confirmation, and makes its held removals due at once so the next batches capture them
    /// instead of waiting for their scrape interval.
    pub async fn confirm_mass_removal(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        hours: MassRemovalConfirmationHours,
    ) -> Result<MassRemovalConfirmation, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let confirmed_until = sqlx::query_scalar::<_, OffsetDateTime>(
            "UPDATE listing_sources \
             SET mass_removal_confirmed_until = NOW() + make_interval(hours => $2), \
                 mass_removal_confirmed_by = $3, \
                 updated = NOW() \
             WHERE listing_source_id = $1 \
             RETURNING mass_removal_confirmed_until",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(i32::try_from(hours.get()).unwrap_or(i32::MAX))
        .bind(actor.as_str())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::ListingSourceNotFound(
            *listing_source_id,
        ))?;
        let rearmed_held_urls = sqlx::query(
            "UPDATE listing_source_urls \
             SET last_scraped = NULL, updated = NOW() \
             WHERE listing_source_id = $1 \
               AND url_class = 'product' \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
               AND last_error_kind = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(MASS_REMOVAL_HELD_ERROR_KIND)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        transaction.commit().await?;
        info!(
            listing_source_id = %listing_source_id,
            actor = actor.as_str(),
            confirmed_until = %confirmed_until,
            hours = hours.get(),
            rearmed_held_urls,
            "Confirmed the ListingSource's mass removal; its held removals are due at once"
        );
        Ok(MassRemovalConfirmation {
            mass_removal_confirmed_until: confirmed_until,
            mass_removal_confirmed_by: actor,
            rearmed_held_urls,
        })
    }

    /// Revokes the ListingSource's confirmation, so the guard holds its removals again from the
    /// next batch; revoking a source without one changes nothing and succeeds.
    pub async fn revoke_mass_removal_confirmation(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<(), ReviewRepositoryError> {
        sqlx::query_scalar::<_, bool>(
            "UPDATE listing_sources \
             SET mass_removal_confirmed_until = NULL, mass_removal_confirmed_by = NULL, \
                 updated = NOW() \
             WHERE listing_source_id = $1 \
             RETURNING TRUE",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::ListingSourceNotFound(
            *listing_source_id,
        ))?;
        info!(
            listing_source_id = %listing_source_id,
            actor = actor.as_str(),
            "Revoked the ListingSource's mass-removal confirmation; the guard holds its removals again"
        );
        Ok(())
    }
}
