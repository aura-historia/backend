use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, current_postgres_credentials, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{sync::Arc, time::Instant};
use user_postgres::SqlxMarketingConsentIntentRepository;
use user_postgres::{SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::RegisterCognitoUserHandler;

const COMPONENT: &str = "cognito-post-confirmation";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();

    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let services = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let services = Arc::clone(&services);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let credentials =
                    current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let service = services
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .connect(COMPONENT, credentials.credentials())
                            .await?;
                        Ok::<_, Error>(RegisterCognitoUserHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserRepositoryFactory::new(),
                            SqlxUserCognitoIdentityRegistryFactory::new(),
                            SqlxMarketingConsentIntentRepository::new(),
                        ))
                    })
                    .await?;

                handler(event, service.value()).await
            }
        },
    ))
    .await
}
