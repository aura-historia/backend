use listing_source_service::ports::{ListingSourceRepository, ListingSourceRepositoryFactory};
use platform_postgres::SqlxTransaction;

use crate::repositories::SqlxListingSourceRepository;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxListingSourceRepositoryFactory;

impl SqlxListingSourceRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ListingSourceRepositoryFactory<SqlxTransaction> for SqlxListingSourceRepositoryFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ListingSourceRepository + 'a {
        SqlxListingSourceRepository {
            connection: tx.connection(),
        }
    }
}
