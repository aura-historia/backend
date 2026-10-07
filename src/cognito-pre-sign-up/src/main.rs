use aws_config::BehaviorVersion;
use aws_lambda_events::cognito::CognitoEventUserPoolsPreSignup;
use cognito_pre_sign_up::{handler, is_external_provider_signup, parse_provider_signup_policy};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, log_cold_start, log_invocation_start,
    logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::timeout;
use user_cognito::CognitoAccountLinker;
use user_postgres::SqlxFederatedAccountReader;
use user_service::use_cases::ResolveFederatedAccountHandler;

const INVOCATION_DEADLINE: Duration = Duration::from_millis(4_200);

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let policy_json = required_config_from_env("COGNITO_PROVIDER_SIGNUP_POLICY")
        .map_err(|_| Error::from("Cognito provider sign-up policy unavailable"))?;
    let providers = Arc::new(
        parse_provider_signup_policy(&policy_json)
            .map_err(|_| Error::from("invalid Cognito provider sign-up policy"))?,
    );
    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let services = Arc::new(VersionedCompositionCache::new());
    let aws_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let linker = Arc::new(CognitoAccountLinker::new(
        aws_sdk_cognitoidentityprovider::Client::new(&aws_config),
    ));

    log_cold_start("cognito-pre-sign-up", initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPreSignup>| {
            let providers = Arc::clone(&providers);
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let services = Arc::clone(&services);
            let linker = Arc::clone(&linker);
            async move {
                log_invocation_start("cognito-pre-sign-up", &event.context);
                let request_id = event.context.request_id.clone();
                let invocation_started_at = Instant::now();
                if !is_external_provider_signup(&event.payload) {
                    tracing::info!(
                        request_id,
                        trigger_kind = "non_external",
                        result_category = "no_op",
                        duration_ms = invocation_started_at.elapsed().as_millis() as u64,
                    );
                    return Ok(event.payload);
                }

                let invocation = timeout(INVOCATION_DEADLINE, async {
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
                            Ok::<_, Error>(ResolveFederatedAccountHandler::new(
                                SqlxFederatedAccountReader::new(pool),
                            ))
                        })
                        .await?;

                    match handler(event, providers.as_ref(), service.value(), linker.as_ref()).await
                    {
                        Ok(response) => Ok(response),
                        Err(error) => {
                            tracing::warn!(
                                request_id,
                                trigger_kind = "pre_sign_up",
                                result_category = error.category(),
                            );
                            Err(Error::from(error))
                        }
                    }
                })
                .await;

                match invocation {
                    Ok(Ok(response)) => {
                        tracing::info!(
                            request_id,
                            trigger_kind = "pre_sign_up",
                            result_category = "completed",
                            duration_ms = invocation_started_at.elapsed().as_millis() as u64,
                        );
                        Ok(response)
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(
                            request_id,
                            trigger_kind = "pre_sign_up",
                            result_category = "invocation_failed",
                            duration_ms = invocation_started_at.elapsed().as_millis() as u64,
                        );
                        Err(error)
                    }
                    Err(_) => {
                        tracing::warn!(
                            request_id,
                            trigger_kind = "pre_sign_up",
                            result_category = "invocation_deadline_exceeded",
                            duration_ms = invocation_started_at.elapsed().as_millis() as u64,
                        );
                        Err(Error::from(
                            "Cognito pre-sign-up processing deadline exceeded",
                        ))
                    }
                }
            }
        },
    ))
    .await
}
