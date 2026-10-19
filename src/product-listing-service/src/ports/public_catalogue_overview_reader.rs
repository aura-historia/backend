use application::error::BoxError;

use crate::use_cases::queries::get_public_catalogue_overview::PublicCatalogueOverview;

#[derive(Debug, thiserror::Error)]
pub enum PublicCatalogueOverviewReadError {
    #[error("temporary public catalogue overview read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public catalogue overview read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PublicCatalogueOverviewReader: Send {
    /// Aggregates the public catalogue at one database timestamp, returned as `as_of`.
    async fn read_overview(
        &mut self,
    ) -> Result<PublicCatalogueOverview, PublicCatalogueOverviewReadError>;
}

pub trait PublicCatalogueOverviewReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PublicCatalogueOverviewReader + 'tx;
}
