use backend_cleanup_lambda::{cleanup_batch_size_from_env, run_cleanup};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use oauth_postgres::SqlxExpiredCredentialCleanup;
use oauth_service::use_cases::CleanupExpiredCredentialsAndProviderReceiptsHandler;
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use serde_json::Value;
use std::{sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let batch_size = cleanup_batch_size_from_env()?;
    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let cleanup = Arc::new(VersionedCompositionCache::new());

    log_cold_start("backend-cleanup-lambda", initialization_started_at);
    run(service_fn(move |event: LambdaEvent<Value>| {
        let postgres = postgres.clone();
        let credentials = Arc::clone(&credentials);
        let cleanup = Arc::clone(&cleanup);
        async move {
            log_invocation_start("backend-cleanup-lambda", &event.context);
            let credentials = credentials
                .current()
                .await
                .map_err(|_| Error::from("PostgreSQL credential refresh unavailable"))?;
            let cleanup = cleanup
                .get_or_try_build(credentials.version_id(), || async {
                    let pool = postgres
                        .pool_config(credentials.credentials())
                        .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                        .connect()
                        .await
                        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                    Ok::<_, Error>(CleanupExpiredCredentialsAndProviderReceiptsHandler::new(
                        SqlxExpiredCredentialCleanup::new(pool),
                    ))
                })
                .await?;

            run_cleanup(cleanup.value(), batch_size).await
        }
    }))
    .await
}
