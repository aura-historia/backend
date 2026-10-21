use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;
use user_postgres::{
    SqlxUserAccountReaderFactory, SqlxUserAdminReaderFactory, SqlxUserRepositoryFactory,
};
use user_service::use_cases::{
    commands::update_user_profile::{UpdateUserProfileHandler, UpdateUserProfileUseCase},
    queries::get_own_user::{GetOwnUserHandler, GetOwnUserUseCase},
};

/// Own-account reads and profile edits; credential/account administration is separate.
#[derive(Clone)]
pub struct UserProfile {
    pub get: Arc<dyn GetOwnUserUseCase>,
    pub update: Arc<dyn UpdateUserProfileUseCase>,
}

impl UserProfile {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        Self {
            get: Arc::new(GetOwnUserHandler::new(
                unit_of_work.clone(),
                SqlxUserAccountReaderFactory::new(),
            )),
            update: Arc::new(UpdateUserProfileHandler::new(
                unit_of_work,
                SqlxUserRepositoryFactory::new(),
                SqlxUserAdminReaderFactory::new(),
            )),
        }
    }
}
