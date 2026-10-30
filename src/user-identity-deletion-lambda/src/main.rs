use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaInvocationBudget, LambdaPostgresConfig, VersionedCompositionCache,
    current_postgres_credentials, log_cold_start, log_invocation_start, logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use user_cognito::CognitoUserIdentityDeleter;
use user_postgres::SqlxUserIdentityDeletionStoreFactory;
use user_service::use_cases::commands::delete_user_identity::{
    DeleteUserIdentityHandler, DeleteUserIdentityUseCase,
};

const COMPONENT: &str = "user-identity-deletion-lambda";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let started = Instant::now();
    init(logging_config_from_env());
    let postgres = LambdaPostgresConfig::from_env()?;
    let user_pool_id = std::env::var("AURA_HISTORIA_COGNITO_USER_POOL_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::from("missing Cognito user pool configuration"))?;
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let cognito_config = aws_sdk_cognitoidentityprovider::config::Builder::from(&aws_config)
        .timeout_config(
            aws_sdk_cognitoidentityprovider::config::timeout::TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(3))
                .build(),
        )
        .build();
    let cognito = aws_sdk_cognitoidentityprovider::Client::from_conf(cognito_config);
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credentials unavailable"))?;
    let cache = Arc::new(VersionedCompositionCache::<
        Arc<dyn DeleteUserIdentityUseCase>,
    >::new());
    log_cold_start(COMPONENT, started);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let cache = Arc::clone(&cache);
        let cognito = cognito.clone();
        let user_pool_id = user_pool_id.clone();
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let budget = LambdaInvocationBudget::from_context(
                &event.context,
                Duration::from_secs(45),
                Duration::from_secs(5),
            );
            tokio::time::timeout(budget.remaining(), async move {
                let current = current_postgres_credentials(COMPONENT, credentials.as_ref()).await?;
                let use_case = cache
                    .get_or_try_build(current.version_id(), || async {
                        let pool = postgres.connect(COMPONENT, current.credentials()).await?;
                        Ok::<_, Error>(Arc::new(DeleteUserIdentityHandler::new(
                            SqlxUnitOfWork::new(pool),
                            SqlxUserIdentityDeletionStoreFactory,
                            CognitoUserIdentityDeleter::new(cognito, user_pool_id),
                        ))
                            as Arc<dyn DeleteUserIdentityUseCase>)
                    })
                    .await?;
                user_identity_deletion_lambda::handler(
                    event,
                    use_case.value().as_ref(),
                    budget.remaining(),
                )
                .await
            })
            .await
            .map_err(|_| Error::from("user identity deletion invocation timed out"))?
        }
    }))
    .await
}
