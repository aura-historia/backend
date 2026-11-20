use cognito_post_confirmation_forwarder::Forwarder;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{log_cold_start, log_invocation_start};
use platform_observability::{LoggingConfig, init};
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const COMPONENT: &str = "cognito-post-confirmation-forwarder";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::from_env());
    let forwarder = Arc::new(Forwarder::from_env()?);
    log_cold_start(COMPONENT, initialization_started_at);

    run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let forwarder = Arc::clone(&forwarder);
        async move {
            log_invocation_start(COMPONENT, &event.context);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::from("system clock is before the Unix epoch"))?
                .as_secs()
                .cast_signed();
            match forwarder
                .forward(&event.payload, now, &event.context.request_id)
                .await
            {
                // Cognito expects the event back unchanged.
                Ok(()) => Ok(event.payload),
                Err(error) => {
                    // Only the bounded category: the event holds the user's email.
                    tracing::warn!(
                        request_id = %event.context.request_id,
                        result_category = error.category(),
                        "Cognito post-confirmation forward failed"
                    );
                    Err(Error::from(error))
                }
            }
        }
    }))
    .await
}
