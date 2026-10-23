//! Shipping management requires only PostgreSQL and the existing source access policy.
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use platform_postgres::SqlxUnitOfWork;
use shipping_service::use_cases::{
    AssignShippingDefaultHandler, AssignShippingDefaultUseCase, ClearShippingDefaultHandler,
    ClearShippingDefaultUseCase, CreateShippingProfileHandler, CreateShippingProfileUseCase,
    GetShippingDefaultHandler, GetShippingDefaultUseCase, GetShippingProfileHandler,
    GetShippingProfileUseCase, ListShippingProfilesHandler, ListShippingProfilesUseCase,
    ReplaceShippingProfileHandler, ReplaceShippingProfileUseCase, RestoreShippingProfileHandler,
    RestoreShippingProfileUseCase, RetireShippingProfileHandler, RetireShippingProfileUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct ShippingManagement {
    pub create: Arc<dyn CreateShippingProfileUseCase>,
    pub replace: Arc<dyn ReplaceShippingProfileUseCase>,
    pub retire: Arc<dyn RetireShippingProfileUseCase>,
    pub restore: Arc<dyn RestoreShippingProfileUseCase>,
    pub assign_default: Arc<dyn AssignShippingDefaultUseCase>,
    pub clear_default: Arc<dyn ClearShippingDefaultUseCase>,
    pub get: Arc<dyn GetShippingProfileUseCase>,
    pub list: Arc<dyn ListShippingProfilesUseCase>,
    pub get_default: Arc<dyn GetShippingDefaultUseCase>,
}
impl ShippingManagement {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let uow = SqlxUnitOfWork::new(pool.clone());
        Self {
            create: Arc::new(CreateShippingProfileHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            replace: Arc::new(ReplaceShippingProfileHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            retire: Arc::new(RetireShippingProfileHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            restore: Arc::new(RestoreShippingProfileHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            assign_default: Arc::new(AssignShippingDefaultHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            clear_default: Arc::new(ClearShippingDefaultHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            get: Arc::new(GetShippingProfileHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingReaderFactory,
                SqlxSourceLocationAccessFactory,
            )),
            list: Arc::new(ListShippingProfilesHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingReaderFactory,
                SqlxSourceLocationAccessFactory,
            )),
            get_default: Arc::new(GetShippingDefaultHandler::new(
                uow.clone(),
                shipping_postgres::SqlxShippingReaderFactory,
                SqlxSourceLocationAccessFactory,
            )),
        }
    }
}
