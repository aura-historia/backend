use aws_config::BehaviorVersion;
use aws_lambda_events::cognito::CognitoEventUserPoolsPreSignup;
use cognito_pre_sign_up::{handler, is_external_provider_signup, parse_provider_signup_policy};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LazyComposition, RuntimePostgresConfig, log_cold_start, log_invocation_start,
    required_config_from_env,
};
use platform_observability::{LoggingConfig, init};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::timeout;
use user_cognito::CognitoAccountLinker;
use user_postgres::SqlxFederatedAccountReader;
use user_service::use_cases::ResolveFederatedAccountHandler;

const COMPONENT: &str = "cognito-pre-sign-up";

const INVOCATION_DEADLINE: Duration = Duration::from_millis(4_200);

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::from_env());

    let policy_json = required_config_from_env("COGNITO_PROVIDER_SIGNUP_POLICY")
        .map_err(|_| Error::from("Cognito provider sign-up policy unavailable"))?;
    let providers = Arc::new(
        parse_provider_signup_policy(&policy_json)
            .map_err(|_| Error::from("invalid Cognito provider sign-up policy"))?,
    );
    let postgres = RuntimePostgresConfig::from_env()?;
    let services = Arc::new(LazyComposition::new());
    let aws_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let linker = Arc::new(CognitoAccountLinker::new(
        aws_sdk_cognitoidentityprovider::Client::new(&aws_config),
    ));

    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPreSignup>| {
            let providers = Arc::clone(&providers);
            let postgres = postgres.clone();
            let services = Arc::clone(&services);
            let linker = Arc::clone(&linker);
            async move {
                log_invocation_start(COMPONENT, &event.context);
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
                    let service = services
                        .get_or_try_build(|| async {
                            let pool = postgres.connect(COMPONENT).await?;
                            Ok::<_, Error>(Arc::new(ResolveFederatedAccountHandler::new(
                                SqlxFederatedAccountReader::new(pool),
                            )))
                        })
                        .await?;

                    match handler(event, providers.as_ref(), service.as_ref(), linker.as_ref())
                        .await
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
