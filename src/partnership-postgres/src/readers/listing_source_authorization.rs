use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_service::ports::*;
use sqlx::PgPool;
use user_core::user_id::UserId;
#[derive(Clone)]
pub struct SqlxListingSourceAuthorization {
    pool: PgPool,
}
impl SqlxListingSourceAuthorization {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl ListingSourceAuthorization for SqlxListingSourceAuthorization {
    async fn can_write_source(
        &self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, SourceAuthorizationError> {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM partnership_listing_source_grants WHERE user_id=$1 AND listing_source_id=$2)").bind(uuid::Uuid::from(user_id)).bind(uuid::Uuid::from(listing_source_id)).fetch_one(&self.pool).await.map_err(|e|SourceAuthorizationError::TemporarilyUnavailable{source:box_error(e)})
    }
    async fn list_sources_user_administers(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AdministeredListingSource>, SourceAuthorizationError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            listing_source_id: uuid::Uuid,
            listing_source_slug_id: String,
            name: String,
        }
        let rows=sqlx::query_as::<_,Row>("SELECT s.listing_source_id,s.listing_source_slug_id,s.name FROM partnership_listing_source_grants g JOIN listing_sources s ON s.listing_source_id=g.listing_source_id WHERE g.user_id=$1 ORDER BY s.name").bind(uuid::Uuid::from(user_id)).fetch_all(&self.pool).await.map_err(|e|SourceAuthorizationError::TemporarilyUnavailable{source:box_error(e)})?;
        rows.into_iter()
            .map(|row| {
                Ok(AdministeredListingSource {
                    listing_source_id: ListingSourceId::from(row.listing_source_id),
                    slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|e| {
                        SourceAuthorizationError::InvalidReadModel {
                            source: box_error(e),
                        }
                    })?,
                    name: ListingSourceName::from(row.name),
                })
            })
            .collect()
    }
}
