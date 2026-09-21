use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_secretsmanager::Client;
use platform_postgres::{
    PostgresCredentialRefreshError, PostgresCredentialsProvider, VersionedPostgresCredentials,
};
use serde::Deserialize;
use std::{env, future::Future, sync::Arc, time::Duration};

pub const POSTGRES_SECRET_ARN_ENV: &str = "POSTGRES_SECRET_ARN";
const SECRETS_MANAGER_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Reads the exact runtime PostgreSQL secret. It never mutates or rotates that secret.
struct AwsSecretsManagerPostgresCredentialsProvider {
    client: Client,
    secret_arn: PostgresSecretArn,
}

/// Selects runtime Secrets Manager credentials when an ARN is configured; fixture stages use
/// the existing explicit username/password environment variables instead.
pub async fn postgres_credentials_provider_from_env()
-> Result<Arc<dyn PostgresCredentialsProvider>, PostgresCredentialsProviderConfigError> {
    if env::var_os(POSTGRES_SECRET_ARN_ENV).is_some() {
        let provider = AwsSecretsManagerPostgresCredentialsProvider::from_env()
            .await
            .map_err(PostgresCredentialsProviderConfigError::SecretsManager)?;
        Ok(Arc::new(provider))
    } else {
        let provider = StaticPostgresCredentialsProvider::from_env()
            .map_err(PostgresCredentialsProviderConfigError::Static)?;
        Ok(Arc::new(provider))
    }
}

/// Fixture-only provider. Real Lambda stages always use the AWS adapter above.
struct StaticPostgresCredentialsProvider {
    credentials: VersionedPostgresCredentials,
}

impl StaticPostgresCredentialsProvider {
    fn from_env() -> Result<Self, StaticPostgresCredentialsProviderConfigError> {
        let username = required_fixture_env("POSTGRES_USERNAME")?;
        let password = required_fixture_env("POSTGRES_PASSWORD")?;
        let credentials =
            VersionedPostgresCredentials::new("static".to_owned(), username, password)
                .map_err(|_| StaticPostgresCredentialsProviderConfigError::InvalidCredentials)?;
        Ok(Self { credentials })
    }
}

#[async_trait]
impl PostgresCredentialsProvider for StaticPostgresCredentialsProvider {
    async fn current(
        &self,
    ) -> Result<VersionedPostgresCredentials, PostgresCredentialRefreshError> {
        Ok(self.credentials.clone())
    }
}

fn required_fixture_env(
    name: &'static str,
) -> Result<String, StaticPostgresCredentialsProviderConfigError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => {
            Err(StaticPostgresCredentialsProviderConfigError::Missing)
        }
        Err(env::VarError::NotUnicode(_)) => {
            Err(StaticPostgresCredentialsProviderConfigError::InvalidCredentials)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StaticPostgresCredentialsProviderConfigError {
    #[error("missing fixture PostgreSQL credential configuration")]
    Missing,
    #[error("invalid fixture PostgreSQL credential configuration")]
    InvalidCredentials,
}

impl AwsSecretsManagerPostgresCredentialsProvider {
    async fn from_env() -> Result<Self, SecretsManagerPostgresProviderInitError> {
        let secret_arn = PostgresSecretArn::from_env()?;
        let config = aws_config::defaults(BehaviorVersion::latest()).load().await;

        Ok(Self {
            client: Client::new(&config),
            secret_arn,
        })
    }
}

#[async_trait]
impl PostgresCredentialsProvider for AwsSecretsManagerPostgresCredentialsProvider {
    async fn current(
        &self,
    ) -> Result<VersionedPostgresCredentials, PostgresCredentialRefreshError> {
        let result = bounded_secret_request(
            SECRETS_MANAGER_REQUEST_TIMEOUT,
            self.client
                .get_secret_value()
                .secret_id(self.secret_arn.as_str())
                .version_stage("AWSCURRENT")
                .send(),
        )
        .await?;

        parse_current_secret(result.version_id(), result.secret_string())
            .map_err(|_| PostgresCredentialRefreshError)
    }
}

async fn bounded_secret_request<T, E, Request>(
    timeout: Duration,
    request: Request,
) -> Result<T, PostgresCredentialRefreshError>
where
    Request: Future<Output = Result<T, E>>,
{
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| PostgresCredentialRefreshError)?
        .map_err(|_| PostgresCredentialRefreshError)
}

#[derive(Clone, PartialEq, Eq)]
struct PostgresSecretArn(String);

impl PostgresSecretArn {
    fn from_env() -> Result<Self, SecretsManagerPostgresProviderInitError> {
        match env::var(POSTGRES_SECRET_ARN_ENV) {
            Ok(value) => Self::parse(value),
            Err(env::VarError::NotPresent) => {
                Err(SecretsManagerPostgresProviderInitError::MissingSecretArn)
            }
            Err(env::VarError::NotUnicode(_)) => {
                Err(SecretsManagerPostgresProviderInitError::InvalidSecretArn)
            }
        }
    }

    fn parse(value: String) -> Result<Self, SecretsManagerPostgresProviderInitError> {
        let mut parts = value.splitn(6, ':');
        let is_secret_arn = matches!(
            (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ),
            (
                Some("arn"),
                Some(partition),
                Some("secretsmanager"),
                Some(region),
                Some(account),
                Some(resource),
            ) if !partition.is_empty()
                && !region.is_empty()
                && account.len() == 12
                && account.bytes().all(|byte| byte.is_ascii_digit())
                && resource
                    .strip_prefix("secret:")
                    .is_some_and(|name| !name.is_empty())
        );
        if value.trim() != value || !is_secret_arn {
            return Err(SecretsManagerPostgresProviderInitError::InvalidSecretArn);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PostgresSecretArn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PostgresSecretArn(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretsManagerPostgresProviderInitError {
    #[error("missing PostgreSQL Secrets Manager ARN configuration")]
    MissingSecretArn,
    #[error("invalid PostgreSQL Secrets Manager ARN configuration")]
    InvalidSecretArn,
}

#[derive(Debug, thiserror::Error)]
pub enum PostgresCredentialsProviderConfigError {
    #[error("invalid PostgreSQL Secrets Manager credential provider configuration")]
    SecretsManager(#[source] SecretsManagerPostgresProviderInitError),
    #[error("invalid fixture PostgreSQL credential provider configuration")]
    Static(#[source] StaticPostgresCredentialsProviderConfigError),
}

#[derive(Deserialize)]
struct PostgresSecretPayload {
    username: String,
    password: String,
}

fn parse_current_secret(
    version_id: Option<&str>,
    secret_string: Option<&str>,
) -> Result<VersionedPostgresCredentials, SecretsManagerPostgresSecretError> {
    let version_id = version_id.ok_or(SecretsManagerPostgresSecretError::MissingVersion)?;
    let secret_string =
        secret_string.ok_or(SecretsManagerPostgresSecretError::MissingSecretString)?;
    let payload: PostgresSecretPayload = serde_json::from_str(secret_string)
        .map_err(|_| SecretsManagerPostgresSecretError::InvalidPayload)?;

    VersionedPostgresCredentials::new(version_id.to_owned(), payload.username, payload.password)
        .map_err(|_| SecretsManagerPostgresSecretError::InvalidPayload)
}

#[derive(Debug, thiserror::Error)]
enum SecretsManagerPostgresSecretError {
    #[error("PostgreSQL Secrets Manager response has no version")]
    MissingVersion,
    #[error("PostgreSQL Secrets Manager response has no string secret")]
    MissingSecretString,
    #[error("PostgreSQL Secrets Manager response has invalid credentials")]
    InvalidPayload,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET_ARN: &str = "arn:aws:secretsmanager:eu-central-1:123456789012:secret:/aura-historia/dev/postgres/runtime-AbCdEf";
    const SECRET: &str = r#"{"engine":"postgres","host":"db.example.test","port":5432,"dbname":"aura_historia","username":"aura_runtime","password":"very-secret-password"}"#;

    #[tokio::test]
    async fn should_bound_an_unavailable_secret_refresh_without_exposing_details() {
        let result =
            bounded_secret_request(Duration::ZERO, std::future::pending::<Result<(), ()>>()).await;
        let error = match result {
            Ok(()) => panic!("expected credential refresh timeout"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "PostgreSQL credential refresh unavailable"
        );
    }

    #[test]
    fn should_parse_runtime_credentials_and_preserve_secret_version() {
        let credentials = parse_current_secret(Some("version-2"), Some(SECRET));
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(error) => panic!("expected credentials: {error}"),
        };

        assert_eq!(credentials.version_id(), "version-2");
        assert_eq!(credentials.credentials().username(), "aura_runtime");
        assert_eq!(credentials.credentials().password(), "very-secret-password");
    }

    #[test]
    fn should_reject_missing_or_invalid_secret_fields_without_echoing_secret_data() {
        for (version_id, secret) in [
            (None, Some(SECRET)),
            (Some("version-2"), None),
            (Some("version-2"), Some("not-json")),
            (Some("version-2"), Some(r#"{"username":"aura_runtime"}"#)),
            (
                Some("version-2"),
                Some(r#"{"username":"","password":"secret"}"#),
            ),
        ] {
            let error = match parse_current_secret(version_id, secret) {
                Ok(_) => panic!("expected parsing failure"),
                Err(error) => error,
            };
            let message = error.to_string();

            assert!(!message.contains("very-secret-password"));
            assert!(!message.contains("not-json"));
            assert!(!message.contains("aura_runtime"));
        }
    }

    #[test]
    fn should_require_an_exact_secrets_manager_secret_arn_without_echoing_it() {
        let valid = PostgresSecretArn::parse(SECRET_ARN.to_owned());

        assert!(valid.is_ok());
        for invalid_arn in [
            "runtime-secret-name",
            "arn:aws:s3:eu-central-1:123456789012:secret:runtime",
            "arn:aws:secretsmanager:eu-central-1:not-an-account:secret:runtime",
            "arn:aws:secretsmanager:eu-central-1:123456789012:secret:",
        ] {
            let error = match PostgresSecretArn::parse(invalid_arn.to_owned()) {
                Ok(_) => panic!("expected invalid ARN"),
                Err(error) => error,
            };

            let message = error.to_string();
            assert!(matches!(
                error,
                SecretsManagerPostgresProviderInitError::InvalidSecretArn
            ));
            assert!(!message.contains(invalid_arn));
        }
        assert!(!format!("{:?}", valid).contains(SECRET_ARN));
    }

    #[test]
    fn should_keep_provider_debug_output_redacted() {
        let arn = match PostgresSecretArn::parse(SECRET_ARN.to_owned()) {
            Ok(arn) => arn,
            Err(error) => panic!("expected ARN: {error}"),
        };

        let debug = format!("{arn:?}");

        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(SECRET_ARN));
    }
}
