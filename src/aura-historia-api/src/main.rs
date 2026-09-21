use aura_historia_api::{
    ApiConfig, ApiConfigError, ApiRunError, lambda_app_from_config_and_pool, run_until_shutdown,
};
use platform_lambda_bootstrap::{LambdaPostgresConfig, VersionedCompositionCache, log_cold_start};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const LAMBDA_COMPONENT: &str = "aura-historia-api";
const GOOGLE_ADC_CREDENTIALS_JSON_ENV: &str = "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_ADC_CREDENTIALS_DIRECTORY: &str = "/tmp/aura-historia-google-adc";
const GOOGLE_ADC_CREDENTIALS_FILE_NAME: &str = "application_default_credentials.json";

fn main() -> Result<(), MainError> {
    let initialization_started_at = Instant::now();
    let is_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    if is_lambda {
        materialize_google_application_credentials_from_env()?;
    }
    init(logging_config_from_env());

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(MainError::Runtime)?
        .block_on(async move {
            if is_lambda {
                let config = ApiConfig::from_env()?;
                let postgres = LambdaPostgresConfig::from_env()?;
                let credentials = postgres_credentials_provider_from_env()
                    .await
                    .map_err(|_| MainError::PostgresCredentials)?;
                let routers = Arc::new(VersionedCompositionCache::new());

                log_cold_start(LAMBDA_COMPONENT, initialization_started_at);
                aura_historia_api::lambda::run(move || {
                    let config = config.clone();
                    let postgres = postgres.clone();
                    let credentials = Arc::clone(&credentials);
                    let routers = Arc::clone(&routers);
                    async move {
                        let credentials = credentials
                            .current()
                            .await
                            .map_err(|_| "PostgreSQL credential refresh unavailable".to_owned())?;
                        let router = routers
                            .get_or_try_build(credentials.version_id(), || async {
                                let pool = postgres
                                    .pool_config(credentials.credentials())
                                    .map_err(|_| "invalid PostgreSQL configuration".to_owned())?
                                    .connect()
                                    .await
                                    .map_err(|_| "failed to create PostgreSQL pool".to_owned())?;
                                lambda_app_from_config_and_pool(&config, pool)
                                    .await
                                    .map_err(|_| "failed to compose API router".to_owned())
                            })
                            .await?;
                        Ok::<_, String>(router.value().clone())
                    }
                })
                .await?;
            } else {
                let config = ApiConfig::from_env()?;
                run_until_shutdown(config, shutdown_signal()).await?;
            }

            Ok(())
        })
}

fn materialize_google_application_credentials_from_env()
-> Result<(), GoogleAdcCredentialsMaterializationError> {
    let credentials_json = std::env::var(GOOGLE_ADC_CREDENTIALS_JSON_ENV)
        .map_err(|_| GoogleAdcCredentialsMaterializationError::MissingCredentials)?;
    let credentials_path = materialize_google_application_credentials(
        Some(&credentials_json),
        Path::new(GOOGLE_ADC_CREDENTIALS_DIRECTORY),
    )?;

    // SAFETY: Lambda credential setup runs in synchronous `main` before the Tokio runtime and
    // application work begin, so no other thread can observe a partially changed environment.
    unsafe {
        std::env::set_var(GOOGLE_APPLICATION_CREDENTIALS_ENV, &credentials_path);
        std::env::remove_var(GOOGLE_ADC_CREDENTIALS_JSON_ENV);
    }
    Ok(())
}

fn materialize_google_application_credentials(
    credentials_json: Option<&str>,
    credentials_directory: &Path,
) -> Result<PathBuf, GoogleAdcCredentialsMaterializationError> {
    let credentials_json = credentials_json
        .filter(|value| !value.trim().is_empty())
        .ok_or(GoogleAdcCredentialsMaterializationError::MissingCredentials)?;
    let is_json_object = match serde_json::from_str::<serde_json::Value>(credentials_json) {
        Ok(value) => value.is_object(),
        Err(_) => false,
    };
    if !is_json_object {
        return Err(GoogleAdcCredentialsMaterializationError::InvalidCredentials);
    }

    fs::create_dir_all(credentials_directory)
        .map_err(|_| GoogleAdcCredentialsMaterializationError::PrepareCredentialsFile)?;
    fs::set_permissions(credentials_directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| GoogleAdcCredentialsMaterializationError::PrepareCredentialsFile)?;

    let credentials_path = credentials_directory.join(GOOGLE_ADC_CREDENTIALS_FILE_NAME);
    let mut credentials_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&credentials_path)
        .map_err(|_| GoogleAdcCredentialsMaterializationError::PrepareCredentialsFile)?;
    fs::set_permissions(&credentials_path, fs::Permissions::from_mode(0o600))
        .map_err(|_| GoogleAdcCredentialsMaterializationError::PrepareCredentialsFile)?;
    credentials_file
        .write_all(credentials_json.as_bytes())
        .and_then(|()| credentials_file.sync_all())
        .map_err(|_| GoogleAdcCredentialsMaterializationError::PrepareCredentialsFile)?;

    Ok(credentials_path)
}

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error(transparent)]
    Config(#[from] ApiConfigError),
    #[error(transparent)]
    Run(#[from] ApiRunError),

    #[error(transparent)]
    PostgresConfig(#[from] platform_lambda_bootstrap::LambdaBootstrapConfigError),
    #[error("PostgreSQL credential provider unavailable")]
    PostgresCredentials,
    #[error("failed to build API async runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    GoogleAdcCredentials(#[from] GoogleAdcCredentialsMaterializationError),
    #[error(transparent)]
    Lambda(#[from] lambda_http::Error),
}

#[derive(thiserror::Error, Debug)]
enum GoogleAdcCredentialsMaterializationError {
    #[error("missing Google ADC credentials configuration")]
    MissingCredentials,
    #[error("Google ADC credentials configuration must be a JSON object")]
    InvalidCredentials,
    #[error("failed to prepare private Google ADC credentials file")]
    PrepareCredentialsFile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    const TEST_CREDENTIALS: &str = r#"{"type":"service_account","project_id":"test-project"}"#;

    fn test_credentials_directory() -> PathBuf {
        std::env::temp_dir().join(format!("aura-historia-google-adc-test-{}", Uuid::new_v4()))
    }

    fn remove_test_directory(directory: &Path) {
        if let Err(error) = fs::remove_dir_all(directory) {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
    }

    #[test]
    fn should_materialize_google_adc_credentials_to_a_private_file() {
        let directory = test_credentials_directory();
        let result = materialize_google_application_credentials(Some(TEST_CREDENTIALS), &directory);
        let credentials_path = match result {
            Ok(path) => path,
            Err(error) => panic!("expected test credentials to materialize: {error}"),
        };

        let contents = match fs::read_to_string(&credentials_path) {
            Ok(contents) => contents,
            Err(error) => panic!("expected materialized credentials: {error}"),
        };
        let file_mode = match fs::metadata(&credentials_path) {
            Ok(metadata) => metadata.permissions().mode() & 0o777,
            Err(error) => panic!("expected private credentials file metadata: {error}"),
        };
        let directory_mode = match fs::metadata(&directory) {
            Ok(metadata) => metadata.permissions().mode() & 0o777,
            Err(error) => panic!("expected private credentials directory metadata: {error}"),
        };

        assert_eq!(contents, TEST_CREDENTIALS);
        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);
        remove_test_directory(&directory);
    }

    #[test]
    fn should_redact_missing_or_invalid_google_adc_credentials_configuration() {
        let directory = test_credentials_directory();
        let missing = materialize_google_application_credentials(None, &directory);
        let invalid_input = "{not-valid-json: test-only-redaction-check}";
        let invalid = materialize_google_application_credentials(Some(invalid_input), &directory);

        assert!(matches!(
            missing,
            Err(GoogleAdcCredentialsMaterializationError::MissingCredentials)
        ));
        assert!(matches!(
            invalid,
            Err(GoogleAdcCredentialsMaterializationError::InvalidCredentials)
        ));
        let invalid_message =
            GoogleAdcCredentialsMaterializationError::InvalidCredentials.to_string();
        assert_eq!(
            invalid_message,
            "Google ADC credentials configuration must be a JSON object"
        );
        assert!(!invalid_message.contains(invalid_input));
        remove_test_directory(&directory);
    }
}
