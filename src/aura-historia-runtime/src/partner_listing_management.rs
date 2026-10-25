use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use partnership_postgres::SqlxListingSourceAuthorization;
use partnership_service::use_cases::queries::list_administered_listing_sources::{
    ListAdministeredListingSourcesHandler, ListAdministeredListingSourcesUseCase,
};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxAdminProductListingReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    commands::partner_product_listings::{
        PartnerProductListingMutationHandler, RestorePartnerProductListingUseCase,
        UpdatePartnerProductListingUseCase, WithdrawPartnerProductListingUseCase,
    },
    queries::partner_product_listings::{
        GetPartnerProductListingHandler, GetPartnerProductListingUseCase,
        ListPartnerProductListingsHandler, ListPartnerProductListingsUseCase,
    },
};
use sqlx::PgPool;
use std::sync::Arc;

/// Authoritative listing management and granted-source discovery used by REST.
#[derive(Clone)]
pub struct PartnerListingManagement {
    pub sources: Arc<dyn ListAdministeredListingSourcesUseCase>,
    pub get: Arc<dyn GetPartnerProductListingUseCase>,
    pub list: Arc<dyn ListPartnerProductListingsUseCase>,
    pub update: Arc<dyn UpdatePartnerProductListingUseCase>,
    pub withdraw: Arc<dyn WithdrawPartnerProductListingUseCase>,
    pub restore: Arc<dyn RestorePartnerProductListingUseCase>,
}
impl PartnerListingManagement {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let authorization = SqlxListingSourceAuthorization::new(pool.clone());
        let reader = SqlxAdminProductListingReader::new(pool.clone());
        let mutation = Arc::new(PartnerProductListingMutationHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            authorization.clone(),
            SqlxAuctionReferenceValidatorFactory::new(),
        ));
        Self {
            sources: Arc::new(ListAdministeredListingSourcesHandler::new(
                authorization.clone(),
            )),
            get: Arc::new(GetPartnerProductListingHandler::new(
                reader.clone(),
                authorization.clone(),
            )),
            list: Arc::new(ListPartnerProductListingsHandler::new(
                reader,
                authorization,
            )),
            update: mutation.clone(),
            withdraw: mutation.clone(),
            restore: mutation,
        }
    }
}
