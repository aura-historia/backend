use geo_postgres::SqlxResolutionRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;
use user_postgres::SqlxAddressBookFactory;
use user_service::address_book::{AddressBookCommands, AddressBookHandler, AddressBookQueries};

/// Private address-book and browsing-destination use cases; no geocoder is constructed.
#[derive(Clone)]
pub struct PrivateAddresses {
    pub commands: Arc<dyn AddressBookCommands>,
    pub queries: Arc<dyn AddressBookQueries>,
}

impl PrivateAddresses {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let handler = Arc::new(AddressBookHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxAddressBookFactory,
            SqlxResolutionRepositoryFactory,
        ));
        Self {
            commands: handler.clone(),
            queries: handler,
        }
    }
}
