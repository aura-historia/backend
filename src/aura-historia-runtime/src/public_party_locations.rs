use party_postgres::discovery::SqlxPartyLocationDiscoveryReaderFactory;
use party_service::discovery::{DiscoverPartyLocationsHandler, DiscoverPartyLocationsUseCase};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct PublicPartyLocations {
    pub discover: Arc<dyn DiscoverPartyLocationsUseCase>,
}

impl PublicPartyLocations {
    pub(super) fn compose(pool: &PgPool) -> Self {
        Self {
            discover: Arc::new(DiscoverPartyLocationsHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxPartyLocationDiscoveryReaderFactory,
            )),
        }
    }
}
