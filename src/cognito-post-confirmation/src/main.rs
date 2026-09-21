use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use std::time::Instant;
use user_postgres::{SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::RegisterCognitoUserHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let pool = LambdaPostgresConfig::from_env()?
        .into_pool_config()
        .connect()
        .await
        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );

    log_cold_start("cognito-post-confirmation", initialization_started_at);

    run(service_fn(
        |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| async {
            log_invocation_start("cognito-post-confirmation", &event.context);
            handler(event, &service).await
        },
    ))
    .await
}
