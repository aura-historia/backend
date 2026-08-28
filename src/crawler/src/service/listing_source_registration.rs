//! Synchronizes crawler-local ListingSource crawl eligibility from business truth.

use async_trait::async_trait;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use tracing::info;

/// Crawler-safe business identity.
///
/// The business reader supplies canonical ListingSource values. `crawl_enabled` is
/// crawler-local state derived from the complete successful `WEB_CRAWL` source read.
#[derive(Debug, Clone)]
pub struct RegisteredListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub crawl_enabled: bool,
}

/// Reads the authoritative crawler scope.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationSource: Send + Sync {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError>;
}

/// Writes crawler-local ListingSource identity and crawl eligibility.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationRepository: Send + Sync {
    async fn upsert_listing_source(
        &self,
        listing_source: &RegisteredListingSource,
    ) -> Result<(), sqlx::Error>;
    async fn disable_listing_sources_not_in(
        &self,
        crawl_enabled_listing_source_ids: &[ListingSourceId],
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
        let crawl_enabled_listing_source_ids = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .map(|listing_source| listing_source.listing_source_id)
            .collect::<Vec<_>>();

        for listing_source in &listing_sources {
            self.repository
                .upsert_listing_source(listing_source)
                .await?;
        }

        let disabled = self
            .repository
            .disable_listing_sources_not_in(&crawl_enabled_listing_source_ids)
            .await?;
        if disabled > 0 {
            info!(
                disabled,
                "disabled crawler-local listing sources absent from business crawl scope"
            );
        }

        Ok(crawl_enabled_listing_source_ids.len())
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
                (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
             VALUES ($1, $2, $3, $4, NOW(), NOW()) \
             ON CONFLICT (listing_source_id) DO UPDATE SET \
                listing_source_name = EXCLUDED.listing_source_name, \
                listing_source_slug = EXCLUDED.listing_source_slug, \
                crawl_enabled = EXCLUDED.crawl_enabled, \
                updated = NOW()",
        )
        .bind(uuid::Uuid::from(listing_source.listing_source_id))
        .bind(listing_source.listing_source_name.as_ref())
        .bind(listing_source.listing_source_slug.to_string())
        .bind(listing_source.crawl_enabled)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn disable_listing_sources_not_in(
        &self,
        crawl_enabled_listing_source_ids: &[ListingSourceId],
    ) -> Result<u64, sqlx::Error> {
        let ids = crawl_enabled_listing_source_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let result = sqlx::query(
            "UPDATE listing_sources \
             SET crawl_enabled = FALSE, updated = NOW() \
             WHERE crawl_enabled = TRUE \
               AND NOT (listing_source_id = ANY($1::uuid[]))",
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
            listing_source_name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            listing_source_slug: ListingSourceSlugId::from("source"),
            crawl_enabled: true,
        }
    }

    #[tokio::test]
    async fn should_disable_only_sources_absent_from_successful_sync() {
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
            .expect_disable_listing_sources_not_in()
            .times(1)
            .withf(|ids| ids.len() == 1)
            .returning(|_| Box::pin(async { Ok(0) }));

        let count = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .sync()
            .await;
        assert!(matches!(count, Ok(1)));
    }

    #[tokio::test]
    async fn should_not_disable_when_business_read_fails() {
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
