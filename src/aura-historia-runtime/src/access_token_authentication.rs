use sqlx::PgPool;
use std::sync::Arc;
use user_postgres::SqlxAccessTokenAuthenticationReader;
use user_service::use_cases::{AuthenticateAccessTokenHandler, AuthenticateAccessTokenUseCase};

pub(super) fn compose(pool: &PgPool) -> Arc<dyn AuthenticateAccessTokenUseCase> {
    Arc::new(AuthenticateAccessTokenHandler::new(
        SqlxAccessTokenAuthenticationReader::new(pool.clone()),
    ))
}
