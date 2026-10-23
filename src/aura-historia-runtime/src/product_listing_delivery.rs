//! Existing versioned delivery use cases, composed once for the current pool.
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    delivery::{
        SqlxProductListingDeliveryReaderFactory, SqlxProductListingDeliveryReferencesFactory,
    },
};
use product_listing_service::use_cases::{
    ChangeProductListingDeliveryHandler, ChangeProductListingDeliveryUseCase,
    GetProductListingDeliveryHandler, GetProductListingDeliveryUseCase,
};
use std::sync::Arc;
#[derive(Clone)]
pub struct ProductListingDelivery {
    pub get: Arc<dyn GetProductListingDeliveryUseCase>,
    pub change: Arc<dyn ChangeProductListingDeliveryUseCase>,
}
impl ProductListingDelivery {
    pub(super) fn compose(pool: &sqlx::PgPool) -> Self {
        let uow = SqlxUnitOfWork::new(pool.clone());
        Self {
            get: Arc::new(GetProductListingDeliveryHandler::new(
                uow.clone(),
                SqlxProductListingDeliveryReaderFactory,
                SqlxProductListingDeliveryReferencesFactory,
            )),
            change: Arc::new(ChangeProductListingDeliveryHandler::new(
                uow,
                SqlxProductListingRepositoryFactory,
                SqlxProductListingEventAppenderFactory,
                SqlxProductListingDeliveryReferencesFactory,
            )),
        }
    }
}
