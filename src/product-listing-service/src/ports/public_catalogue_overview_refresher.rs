use application::error::BoxError;
use std::time::Duration;

use crate::use_cases::commands::refresh_public_catalogue_overview::PublicCatalogueOverviewRefresh;

#[derive(Debug, thiserror::Error)]
pub enum PublicCatalogueOverviewRefreshError {
    #[error("temporary public catalogue overview refresh failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public catalogue overview calculation")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PublicCatalogueOverviewRefresher: Send {
    /// Returns `None` without aggregating while the stored calculation is younger than
    /// `max_age` by the database clock. Otherwise aggregates the public catalogue at one
    /// database timestamp and stores it unless a newer calculation is already stored.
    async fn refresh_if_older_than(
        &mut self,
        max_age: Duration,
    ) -> Result<Option<PublicCatalogueOverviewRefresh>, PublicCatalogueOverviewRefreshError>;
}

pub trait PublicCatalogueOverviewRefresherFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PublicCatalogueOverviewRefresher + 'tx;
}
