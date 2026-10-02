use aws_config::BehaviorVersion;
use aws_lambda_events::cognito::CognitoEventUserPoolsPreSignup;
use cognito_pre_sign_up::{handler, parse_linking_policy};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    log_cold_start, log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use std::{sync::Arc, time::Instant};
use user_cognito::CognitoAccountLinker;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let policy_json = required_config_from_env("COGNITO_IDENTITY_PROVIDER_LINKING_POLICY")
        .map_err(|_| Error::from("Cognito identity-provider linking policy unavailable"))?;
    let providers = Arc::new(
        parse_linking_policy(&policy_json)
            .map_err(|_| Error::from("invalid Cognito identity-provider linking policy"))?,
    );
    let aws_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let linker = Arc::new(CognitoAccountLinker::new(
        aws_sdk_cognitoidentityprovider::Client::new(&aws_config),
    ));

    log_cold_start("cognito-pre-sign-up", initialization_started_at);

    run(service_fn(
        move |event: LambdaEvent<CognitoEventUserPoolsPreSignup>| {
            let providers = Arc::clone(&providers);
            let linker = Arc::clone(&linker);
            async move {
                log_invocation_start("cognito-pre-sign-up", &event.context);
                let request_id = event.context.request_id.clone();
                match handler(event, providers.as_ref(), linker.as_ref()).await {
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
            }
        },
    ))
    .await
}
