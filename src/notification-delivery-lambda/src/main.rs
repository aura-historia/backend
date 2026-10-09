use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::Client as SesClient;
use aws_smithy_types::timeout::TimeoutConfig;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification_delivery_lambda::{
    compose_delivery_use_case, handler_with_invocation_budget, invocation_budget,
};
use notification_email_aws::EmailDeliveryConfig;
use notification_service::use_cases::commands::deliver_notification::DeliverNotificationUseCase;
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::{future::Future, sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let email = email_config()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .operation_attempt_timeout(std::time::Duration::from_secs(20))
                .operation_timeout(std::time::Duration::from_secs(30))
                .build(),
        )
        .load()
        .await;
    let s3 = S3Client::new(&aws_config);
    let ses = SesClient::new(&aws_config);
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start("notification-delivery-lambda", initialization_started_at);
    run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let postgres = postgres.clone();
        let email = email.clone();
        let credentials = Arc::clone(&credentials);
        let s3 = s3.clone();
        let ses = ses.clone();
        let use_cases = Arc::clone(&use_cases);
        async move {
            log_invocation_start("notification-delivery-lambda", &event.context);
            handle_invocation(event, move || async move {
                let credentials = credentials
                    .current()
                    .await
                    .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
                use_cases
                    .get_or_try_build(credentials.version_id(), || async {
                        let pool = postgres
                            .pool_config(credentials.credentials())
                            .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                            .connect()
                            .await
                            .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                        compose_delivery_use_case(pool, s3, ses, email)
                    })
                    .await
            })
            .await
        }
    }))
    .await
}

type DeliveryUseCase = Arc<dyn DeliverNotificationUseCase>;
type DeliveryUseCaseLease = VersionedCompositionLease<DeliveryUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<DeliveryUseCaseLease, Error>>,
{
    handle_sqs_invocation(
        event,
        "notification-delivery-lambda",
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
}

fn email_config() -> Result<EmailDeliveryConfig, Error> {
    email_config_from_env(std::env::var)
}

fn email_config_from_env(
    get_env: impl Fn(&'static str) -> Result<String, std::env::VarError>,
) -> Result<EmailDeliveryConfig, Error> {
    let required_env =
        |name| get_env(name).map_err(|_| Error::from(format!("missing required {name}")));
    Ok(EmailDeliveryConfig::new(
        required_env("S3_BUCKET_NAME_TEMPLATES")?,
        required_env("NOTIFICATION_EMAIL_FROM")?,
        required_env("NOTIFICATION_EMAIL_REPLY_TO")?,
        required_env("NOTIFICATION_EMAIL_CONFIGURATION_SET")?,
        required_env("STAGE")?,
        required_env("COMMIT_SHA")?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notification_email_aws::EmailDeliveryConfigError;

    const CONFIGURATION_SET_ENV: &str = "NOTIFICATION_EMAIL_CONFIGURATION_SET";

    fn test_env(name: &'static str) -> Result<String, std::env::VarError> {
        Ok(match name {
            "S3_BUCKET_NAME_TEMPLATES" => "test-templates",
            "NOTIFICATION_EMAIL_FROM" => "from@example.test",
            "NOTIFICATION_EMAIL_REPLY_TO" => "reply@example.test",
            CONFIGURATION_SET_ENV => "aura-historia-test-email",
            "STAGE" => "test",
            "COMMIT_SHA" => "test-commit",
            _ => return Err(std::env::VarError::NotPresent),
        }
        .to_owned())
    }

    #[test]
    fn should_require_configuration_set_env_without_falling_back_to_stage() {
        let error = email_config_from_env(|name| {
            if name == CONFIGURATION_SET_ENV {
                Err(std::env::VarError::NotPresent)
            } else {
                test_env(name)
            }
        })
        .err()
        .expect("configuration set is required");
        assert_eq!(
            "missing required NOTIFICATION_EMAIL_CONFIGURATION_SET",
            error.to_string()
        );
    }

    #[test]
    fn should_validate_configuration_set_env_without_exposing_invalid_values() {
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "private@example.test".to_owned(),
            "email.name".to_owned(),
            "émail".to_owned(),
            "a".repeat(65),
        ] {
            let error = email_config_from_env(|name| {
                if name == CONFIGURATION_SET_ENV {
                    Ok(value.clone())
                } else {
                    test_env(name)
                }
            })
            .err()
            .expect("invalid configuration set");
            assert_eq!(
                Some(&EmailDeliveryConfigError::InvalidConfigurationSetName),
                error.downcast_ref::<EmailDeliveryConfigError>()
            );
            assert_eq!(
                "invalid notification email SES configuration set name",
                error.to_string()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn should_reject_non_unicode_configuration_set_env_without_exposing_values() {
        use std::os::unix::ffi::OsStringExt;

        let error = email_config_from_env(|name| {
            if name == CONFIGURATION_SET_ENV {
                Err(std::env::VarError::NotUnicode(
                    std::ffi::OsString::from_vec(vec![0xff]),
                ))
            } else {
                test_env(name)
            }
        })
        .err()
        .expect("configuration set must be Unicode");
        assert_eq!(
            "missing required NOTIFICATION_EMAIL_CONFIGURATION_SET",
            error.to_string()
        );
    }

    #[test]
    fn should_accept_explicit_configuration_set_env_independently_of_stage() {
        for value in [
            "aura-historia-test-email".to_owned(),
            "Custom_Configuration-123".to_owned(),
            "a".repeat(64),
        ] {
            assert!(
                email_config_from_env(|name| {
                    if name == CONFIGURATION_SET_ENV {
                        Ok(value.clone())
                    } else {
                        test_env(name)
                    }
                })
                .is_ok()
            );
        }
    }
}
