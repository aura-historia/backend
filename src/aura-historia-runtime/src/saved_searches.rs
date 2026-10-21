//! Typed saved-search inbound handles shared by REST and MCP.
use embedding::EmbeddingGenerator;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxProductListingDetailsBatchReader;
use product_listing_service::shipping::ShippingEnrichment;
use search_filter_postgres::{
    SqlxSearchFilterMatchRepositoryFactory, SqlxSearchFilterQuotaReaderFactory,
    SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::use_cases::*;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct SavedSearches {
    pub list: Arc<dyn ListOwnedSearchFiltersUseCase>,
    pub create: Arc<dyn CreateSearchFilterUseCase>,
    pub get: Arc<dyn GetOwnedSearchFilterUseCase>,
    pub update: Arc<dyn UpdateOwnedSearchFilterUseCase>,
    pub delete: Arc<dyn DeleteOwnedSearchFilterUseCase>,
    pub matches: Arc<dyn ListSearchFilterMatchesUseCase>,
    pub feedback: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
}
impl SavedSearches {
    pub(super) fn compose(
        pool: &PgPool,
        shipping: ShippingEnrichment,
        embeddings: Arc<dyn EmbeddingGenerator>,
    ) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let reader = SqlxSearchFilterReader::new(pool.clone());
        let addresses = Arc::new(user_service::address_book::AddressBookHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxAddressBookFactory,
            geo_postgres::SqlxResolutionRepositoryFactory,
        ));
        Self {
            list: Arc::new(ListOwnedSearchFiltersHandler::new(reader.clone())),
            create: Arc::new(
                CreateSearchFilterHandler::new(
                    unit_of_work.clone(),
                    SqlxSearchFilterRepositoryFactory,
                    Arc::clone(&embeddings),
                    SqlxSearchFilterQuotaReaderFactory,
                    user_postgres::SqlxUserTierEntitlementsFactory::new(),
                )
                .with_shipping_destinations(addresses.clone()),
            ),
            get: Arc::new(GetOwnedSearchFilterHandler::new(reader.clone())),
            update: Arc::new(
                UpdateOwnedSearchFilterHandler::new(
                    unit_of_work.clone(),
                    SqlxSearchFilterRepositoryFactory,
                    embeddings,
                    reader.clone(),
                    SqlxSearchFilterQuotaReaderFactory,
                    user_postgres::SqlxUserTierEntitlementsFactory::new(),
                )
                .with_shipping_destinations(addresses),
            ),
            delete: Arc::new(DeleteOwnedSearchFilterHandler::new(
                unit_of_work.clone(),
                SqlxSearchFilterRepositoryFactory,
            )),
            matches: Arc::new(ListSearchFilterMatchesHandler::new(
                unit_of_work.clone(),
                reader,
                SqlxProductListingDetailsBatchReader::new(pool.clone()),
                SqlxFxRateSnapshotRepositoryFactory,
                shipping,
            )),
            feedback: Arc::new(UpdateSearchFilterMatchFeedbackHandler::new(
                unit_of_work,
                SqlxSearchFilterRepositoryFactory,
                SqlxSearchFilterMatchRepositoryFactory,
            )),
        }
    }
}
