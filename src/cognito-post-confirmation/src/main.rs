use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{sync::Arc, time::Instant};
use user_postgres::SqlxMarketingConsentIntentRepository;
use user_postgres::{SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::RegisterCognitoUserHandler;

fn validate_backend_release_sha(value: Option<&str>) -> Result<(), Error> {
    match value {
        Some(sha) if sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) => Ok(()),
        _ => Err(Error::from(
            "BACKEND_RELEASE_SHA must be a 40-character hexadecimal SHA",
        )),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    validate_backend_release_sha(std::env::var("BACKEND_RELEASE_SHA").ok().as_deref())?;
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let services = Arc::new(VersionedCompositionCache::new());

    log_cold_start("cognito-post-confirmation", initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let services = Arc::clone(&services);
            async move {
                log_invocation_start("cognito-post-confirmation", &event.context);
                let credentials = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credential refresh unavailable"))?;
                let service = services
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .pool_config(credentials.credentials())
                            .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                            .connect()
                            .await
                            .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_or_invalid_backend_release_sha_before_invocation() {
        assert!(validate_backend_release_sha(Some(&"aB01234567".repeat(4))).is_ok());
        for value in [
            None,
            Some(""),
            Some("a"),
            Some(&"a".repeat(39)),
            Some(&"a".repeat(41)),
            Some(&format!("{}g", "a".repeat(39))),
            Some(&format!("{} ", "a".repeat(39))),
        ] {
            assert!(validate_backend_release_sha(value).is_err());
        }
    }
}
