//! Synchronizes crawler-local ListingSource presence from business truth.

use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use tracing::{error, info};

/// Crawler-safe business identity. It deliberately contains no Party, address, or provider data.
#[derive(Debug, Clone)]
pub struct RegisteredListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub listing_source_slug: String,
    pub present: bool,
}

/// Reads the authoritative crawler scope.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationSource: Send + Sync {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError>;
}

/// Writes only crawler-local ListingSource presence metadata.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationRepository: Send + Sync {
    async fn upsert_listing_source(
        &self,
        listing_source: &RegisteredListingSource,
    ) -> Result<(), sqlx::Error>;
    async fn delete_listing_sources_not_in(
        &self,
        present_listing_source_ids: &[ListingSourceId],
    ) -> Result<u64, sqlx::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceSyncError {
    #[error("failed to read registered listing sources: {0}")]
    FetchError(String),
    #[error("crawler-local database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

pub struct ListingSourceRegistrationService {
    source: Box<dyn ListingSourceRegistrationSource>,
    repository: Box<dyn ListingSourceRegistrationRepository>,
}

impl ListingSourceRegistrationService {
    pub fn new(
        source: Box<dyn ListingSourceRegistrationSource>,
        repository: Box<dyn ListingSourceRegistrationRepository>,
    ) -> Self {
        Self { source, repository }
    }

    #[tracing::instrument(name = "listing_source_registration_sync", skip(self))]
    pub async fn sync(&self) -> Result<usize, ListingSourceSyncError> {
        let listing_sources = self.source.fetch_registered_listing_sources().await?;
        let present_listing_source_ids = listing_sources
            .iter()
            .filter(|listing_source| listing_source.present)
            .map(|listing_source| listing_source.listing_source_id)
            .collect::<Vec<_>>();

        for listing_source in &listing_sources {
            if let Err(error) = self.repository.upsert_listing_source(listing_source).await {
                error!(
                    listing_source_id = %listing_source.listing_source_id,
                    error = %error,
                    "failed to sync crawler-local listing source"
                );
            }
        }

        let deleted = self
            .repository
            .delete_listing_sources_not_in(&present_listing_source_ids)
            .await?;
        if deleted > 0 {
            info!(
                deleted,
                "deleted crawler-local listing sources absent from business sync"
            );
        }

        Ok(present_listing_source_ids.len())
    }
}

pub struct ListingSourceRegistrationRepositoryImpl {
    pool: PgPool,
}

impl ListingSourceRegistrationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListingSourceRegistrationRepository for ListingSourceRegistrationRepositoryImpl {
    async fn upsert_listing_source(
        &self,
        listing_source: &RegisteredListingSource,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO listing_sources \
                (listing_source_id, listing_source_name, listing_source_slug, present, created, updated) \
             VALUES ($1, $2, $3, $4, NOW(), NOW()) \
             ON CONFLICT (listing_source_id) DO UPDATE SET \
                listing_source_name = EXCLUDED.listing_source_name, \
                listing_source_slug = EXCLUDED.listing_source_slug, \
                present = EXCLUDED.present, \
                updated = NOW()",
        )
        .bind(uuid::Uuid::from(listing_source.listing_source_id))
        .bind(&listing_source.listing_source_name)
        .bind(&listing_source.listing_source_slug)
        .bind(listing_source.present)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn delete_listing_sources_not_in(
        &self,
        present_listing_source_ids: &[ListingSourceId],
    ) -> Result<u64, sqlx::Error> {
        let ids = present_listing_source_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let result = sqlx::query(
            "DELETE FROM listing_sources \
             WHERE NOT (listing_source_id = ANY($1::uuid[]))",
        )
        .bind(ids)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source() -> RegisteredListingSource {
        RegisteredListingSource {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: "Source".to_owned(),
            listing_source_slug: "source".to_owned(),
            present: true,
        }
    }

    #[tokio::test]
    async fn should_delete_only_sources_absent_from_successful_sync() {
        let source_listing_source = listing_source();
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(move || {
                let source = source_listing_source.clone();
                Box::pin(async move { Ok(vec![source]) })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_upsert_listing_source()
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_delete_listing_sources_not_in()
            .times(1)
            .withf(|ids| ids.len() == 1)
            .returning(|_| Box::pin(async { Ok(0) }));

        let count = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .sync()
            .await;
        assert!(matches!(count, Ok(1)));
    }

    #[tokio::test]
    async fn should_not_delete_when_business_read_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError("unavailable".to_owned()))
                })
            });

        let result = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        )
        .sync()
        .await;
        assert!(matches!(result, Err(ListingSourceSyncError::FetchError(_))));
    }
}
