use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxPublicCatalogueOverviewReaderFactory;
use product_listing_service::use_cases::{
    GetPublicCatalogueOverviewHandler, GetPublicCatalogueOverviewUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct PublicOverview {
    pub get: Arc<dyn GetPublicCatalogueOverviewUseCase>,
}

impl PublicOverview {
    pub(super) fn compose(pool: &PgPool) -> Self {
        Self {
            get: Arc::new(GetPublicCatalogueOverviewHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxPublicCatalogueOverviewReaderFactory::new(),
            )),
        }
    }
}
