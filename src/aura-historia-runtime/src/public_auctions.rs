use auction_postgres::{SqlxAuctionDirectoryReader, SqlxPublicAuctionDetailsReader};
use auction_service::use_cases::{
    GetPublicAuctionHandler, GetPublicAuctionUseCase, ListAuctionsHandler, ListAuctionsUseCase,
};
use product_listing_service::{
    shipping::ShippingEnrichment,
    use_cases::{GetAuctionCatalogueHandler, GetAuctionCatalogueUseCase},
};
use sqlx::PgPool;
use std::sync::Arc;

/// Public auction discovery and catalogue use cases shared by REST and MCP.
#[derive(Clone)]
pub struct PublicAuctions {
    pub get: Arc<dyn GetPublicAuctionUseCase>,
    pub list: Arc<dyn ListAuctionsUseCase>,
    pub catalogue: Arc<dyn GetAuctionCatalogueUseCase>,
}
impl PublicAuctions {
    pub(super) fn compose(pool: &PgPool, shipping: ShippingEnrichment) -> Self {
        Self {
            get: Arc::new(GetPublicAuctionHandler::new(
                SqlxPublicAuctionDetailsReader::new(pool.clone()),
            )),
            list: Arc::new(ListAuctionsHandler::new(SqlxAuctionDirectoryReader::new(
                pool.clone(),
            ))),
            catalogue: Arc::new(GetAuctionCatalogueHandler::new(
                platform_postgres::SqlxUnitOfWork::new(pool.clone()),
                product_listing_postgres::SqlxAuctionCatalogueReaderFactory::new(),
                fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory,
                SqlxPublicAuctionDetailsReader::new(pool.clone()),
                shipping,
            )),
        }
    }
}
