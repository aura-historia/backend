use fxrate_postgres::SqlxFxRateSnapshotReader;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxProductListingHistoryReaderFactory;
use product_listing_service::use_cases::{
    GetProductListingHistoryHandler, GetProductListingHistoryUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;

/// Complete public history uses PostgreSQL and each event's historical FX snapshot.
#[derive(Clone)]
pub struct PublicProductListingHistory {
    pub get: Arc<dyn GetProductListingHistoryUseCase>,
}

impl PublicProductListingHistory {
    pub(super) fn compose(pool: &PgPool) -> Self {
        Self {
            get: Arc::new(GetProductListingHistoryHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingHistoryReaderFactory::new(),
                SqlxFxRateSnapshotReader::new(pool.clone()),
            )),
        }
    }
}
