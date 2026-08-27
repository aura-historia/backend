use application::error::box_error;
use listing_source_core::ListingSourceId;
use partnership_service::ports::*;
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use user_core::user_id::UserId;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxListingSourceGrantRepositoryFactory;
struct Repository<'a> {
    connection: &'a mut PgConnection,
}
impl SqlxListingSourceGrantRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}
impl ListingSourceGrantRepositoryFactory<SqlxTransaction>
    for SqlxListingSourceGrantRepositoryFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ListingSourceGrantRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}
#[async_trait::async_trait]
impl ListingSourceGrantRepository for Repository<'_> {
    async fn grant_source_access(
        &mut self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<(), PartnershipGrantError> {
        sqlx::query("INSERT INTO partnership_listing_source_grants(user_id,listing_source_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(uuid::Uuid::from(user_id)).bind(uuid::Uuid::from(listing_source_id)).execute(&mut*self.connection).await.map(|_|()).map_err(|e|PartnershipGrantError::Internal{source:box_error(e)})
    }
}
