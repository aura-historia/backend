use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use std::{sync::Arc, time::Instant};
use user_postgres::SqlxMarketingConsentIntentRepository;
use user_postgres::{SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::RegisterCognitoUserHandler;

const COMPONENT: &str = "cognito-post-confirmation";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();

    init(logging_config_from_env());

    let postgres = RuntimePostgresConfig::from_env()?;
    let services = Arc::new(LazyComposition::new());

    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| {
            let postgres = postgres.clone();
            let services = Arc::clone(&services);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                let service = services
                    .get_or_try_build(|| async {
                        let pool = postgres.connect(COMPONENT).await?;
                        Ok::<_, Error>(Arc::new(RegisterCognitoUserHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserRepositoryFactory::new(),
                            SqlxUserCognitoIdentityRegistryFactory::new(),
                            SqlxMarketingConsentIntentRepository::new(),
                        )))
                    })
                    .await?;

                handler(event, service.as_ref()).await
            }
        },
    ))
    .await
}
