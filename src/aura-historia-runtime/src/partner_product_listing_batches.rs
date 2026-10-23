use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    CreateProductListingHandler, UpdateProductListingHandler, UpsertProductListingHandler,
    WithdrawProductListingHandler,
    commands::partner_product_listing_batches::{
        PartnerProductListingBatchesHandler, PartnerProductListingBatchesUseCase,
    },
};
use sqlx::PgPool;
use std::sync::Arc;

/// Source-keyed synchronous integration batches, without provider configuration.
#[derive(Clone)]
pub struct PartnerProductListingBatches {
    pub batches: Arc<dyn PartnerProductListingBatchesUseCase>,
}
impl PartnerProductListingBatches {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        Self {
            batches: Arc::new(PartnerProductListingBatchesHandler::new(
                Arc::new(CreateProductListingHandler::new(
                    unit_of_work.clone(),
                    SqlxProductListingRepositoryFactory::new(),
                    SqlxProductListingEventAppenderFactory::new(),
                    SqlxPartnerProductListingAuthorizerFactory::new(),
                    SqlxAuctionReferenceValidatorFactory::new(),
                )),
                Arc::new(UpdateProductListingHandler::new(
                    unit_of_work.clone(),
                    SqlxProductListingRepositoryFactory::new(),
                    SqlxProductListingEventAppenderFactory::new(),
                    SqlxPartnerProductListingAuthorizerFactory::new(),
                    SqlxAuctionReferenceValidatorFactory::new(),
                )),
                Arc::new(UpsertProductListingHandler::new(
                    unit_of_work.clone(),
                    SqlxProductListingRepositoryFactory::new(),
                    SqlxProductListingEventAppenderFactory::new(),
                    SqlxPartnerProductListingAuthorizerFactory::new(),
                    SqlxAuctionReferenceValidatorFactory::new(),
                )),
                Arc::new(WithdrawProductListingHandler::new(
                    unit_of_work,
                    SqlxProductListingRepositoryFactory::new(),
                    SqlxProductListingEventAppenderFactory::new(),
                    SqlxPartnerProductListingAuthorizerFactory::new(),
                )),
            )),
        }
    }
}
