use party_postgres::{
    SqlxPartyLocationAccessFactory, SqlxPartyLocationReaderFactory,
    SqlxPartyLocationRepositoryFactory,
};
use party_service::use_cases::party_locations::*;
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;
use user_postgres::SqlxUserAdminReaderFactory;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;

/// Location management only; grant changes and manager administration remain separate.
#[derive(Clone)]
pub struct PartyLocations {
    pub create: Arc<dyn CreatePartyLocationUseCase>,
    pub update: Arc<dyn UpdatePartyLocationUseCase>,
    pub lifecycle: Arc<dyn SetPartyLocationLifecycleUseCase>,
    pub get: Arc<dyn GetPartyLocationUseCase>,
    pub list: Arc<dyn ListPartyLocationsUseCase>,
}
impl PartyLocations {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let uow = SqlxUnitOfWork::new(pool.clone());
        let admin = || CheckUserAdminHandler::new(uow.clone(), SqlxUserAdminReaderFactory::new());
        Self {
            create: Arc::new(CreatePartyLocationHandler::new(
                uow.clone(),
                SqlxPartyLocationRepositoryFactory,
                SqlxPartyLocationAccessFactory,
                admin(),
            )),
            update: Arc::new(UpdatePartyLocationHandler::new(
                uow.clone(),
                SqlxPartyLocationRepositoryFactory,
                SqlxPartyLocationAccessFactory,
                admin(),
            )),
            lifecycle: Arc::new(SetPartyLocationLifecycleHandler::new(
                uow.clone(),
                SqlxPartyLocationRepositoryFactory,
                SqlxPartyLocationAccessFactory,
                admin(),
            )),
            get: Arc::new(GetPartyLocationHandler::new(
                uow.clone(),
                SqlxPartyLocationReaderFactory,
                SqlxPartyLocationAccessFactory,
                admin(),
            )),
            list: Arc::new(ListPartyLocationsHandler::new(
                uow.clone(),
                SqlxPartyLocationReaderFactory,
                SqlxPartyLocationAccessFactory,
                admin(),
            )),
        }
    }
}
