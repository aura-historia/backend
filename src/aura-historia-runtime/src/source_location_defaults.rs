use listing_source_postgres::{
    SqlxLocationDefaultReaderFactory, SqlxLocationDefaultRepositoryFactory,
    SqlxSourceLocationAccessFactory,
};
use listing_source_service::use_cases::location_default::*;
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;

/// Existing source-operator or administrator policy, independent of location-edit authority.
#[derive(Clone)]
pub struct SourceLocationDefaults {
    pub assign: Arc<dyn AssignLocationDefaultUseCase>,
    pub clear: Arc<dyn ClearLocationDefaultUseCase>,
    pub change: Arc<dyn ChangeLocationDefaultApplicabilityUseCase>,
    pub get: Arc<dyn GetLocationDefaultUseCase>,
}
impl SourceLocationDefaults {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let uow = SqlxUnitOfWork::new(pool.clone());
        Self {
            assign: Arc::new(AssignLocationDefaultHandler::new(
                uow.clone(),
                SqlxLocationDefaultRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            clear: Arc::new(ClearLocationDefaultHandler::new(
                uow.clone(),
                SqlxLocationDefaultRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            change: Arc::new(ChangeLocationDefaultApplicabilityHandler::new(
                uow.clone(),
                SqlxLocationDefaultRepositoryFactory,
                SqlxSourceLocationAccessFactory,
            )),
            get: Arc::new(GetLocationDefaultHandler::new(
                uow,
                SqlxLocationDefaultReaderFactory,
                SqlxSourceLocationAccessFactory,
            )),
        }
    }
}
