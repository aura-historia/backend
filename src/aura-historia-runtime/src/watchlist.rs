use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingDetailsReaderFactory, SqlxProductListingLifecycleGuardFactory,
    SqlxProductListingWatchlistDetailsReaderFactory,
};
use product_listing_service::shipping::ShippingEnrichment;
use sqlx::PgPool;
use std::sync::Arc;
use user_postgres::SqlxUserTierEntitlementsFactory;
use watchlist_postgres::{SqlxWatchlistQuotaReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, ListWatchlistUseCase, UnwatchProductListingHandler,
    UnwatchProductListingUseCase, UpdateWatchlistProductListingHandler,
    UpdateWatchlistProductListingUseCase, WatchProductListingHandler, WatchProductListingUseCase,
};

/// Existing own-watchlist use cases shared by REST and MCP.
#[derive(Clone)]
pub struct Watchlist {
    pub list: Arc<dyn ListWatchlistUseCase>,
    pub add: Arc<dyn WatchProductListingUseCase>,
    pub update: Arc<dyn UpdateWatchlistProductListingUseCase>,
    pub remove: Arc<dyn UnwatchProductListingUseCase>,
}

impl Watchlist {
    pub(super) fn compose(pool: &PgPool, shipping: ShippingEnrichment) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        Self {
            list: Arc::new(ListWatchlistHandler::new(
                unit_of_work.clone(),
                SqlxProductListingWatchlistDetailsReaderFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                shipping.clone(),
            )),
            add: Arc::new(WatchProductListingHandler::new(
                unit_of_work.clone(),
                SqlxWatchlistRepositoryFactory,
                SqlxWatchlistQuotaReaderFactory,
                SqlxUserTierEntitlementsFactory::new(),
                SqlxProductListingLifecycleGuardFactory::new(),
            )),
            update: Arc::new(UpdateWatchlistProductListingHandler::new(
                unit_of_work.clone(),
                SqlxWatchlistRepositoryFactory,
                SqlxWatchlistQuotaReaderFactory,
                SqlxUserTierEntitlementsFactory::new(),
                SqlxProductListingLifecycleGuardFactory::new(),
                SqlxProductListingDetailsReaderFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                shipping,
            )),
            remove: Arc::new(UnwatchProductListingHandler::new(
                unit_of_work,
                SqlxWatchlistRepositoryFactory,
            )),
        }
    }
}
