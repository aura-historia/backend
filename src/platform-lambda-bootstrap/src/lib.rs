mod failure_diagnostic;
mod google_adc;

pub use failure_diagnostic::{
    Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE, UNCLASSIFIED_FAILURE,
};
pub use google_adc::{
    google_application_default_credentials, materialize_google_application_credentials_from_env,
};

use lambda_runtime::Context;
use platform_postgres::{PostgresCredentials, PostgresPoolConfig};
use std::{
    env,
    future::Future,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::sync::OnceCell;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LambdaBootstrapConfigError {
    #[error("missing required Lambda configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid Lambda configuration {name}")]
    Invalid { name: &'static str },
    #[error("invalid Lambda PostgreSQL configuration")]
    InvalidPostgres,
}

#[derive(Clone)]
pub struct LambdaPostgresConfig {
    host: String,
    port: u16,
    database: String,
    max_connections: u32,
    root_certificate: PathBuf,
}

impl LambdaPostgresConfig {
    pub fn from_env() -> Result<Self, LambdaBootstrapConfigError> {
        Ok(Self {
            host: required_env("POSTGRES_HOST")?,
            database: required_env("POSTGRES_DATABASE")?,
            port: optional_env("POSTGRES_PORT", 5432)?,
            max_connections: optional_env("POSTGRES_MAX_CONNECTIONS", 1)?,
            root_certificate: PathBuf::from(required_env("POSTGRES_TLS_ROOT_CERT")?),
        })
    }

    pub fn migration_pool_config(
        &self,
        credentials: &PostgresCredentials,
    ) -> Result<PostgresPoolConfig, LambdaBootstrapConfigError> {
        PostgresPoolConfig::migration(
            self.host.clone(),
            self.port,
            self.database.clone(),
            credentials.username().to_owned(),
            credentials.password().to_owned(),
            self.max_connections,
            self.root_certificate.clone(),
        )
        .map_err(|_| LambdaBootstrapConfigError::InvalidPostgres)
    }
}

/// The runtime PostgreSQL target with the credentials CloudFormation resolved into the function
/// configuration at deployment. Changing a credential takes a release; nothing reads a secret
/// store per invocation. See docs/infra.md#runtime-credentials.
#[derive(Clone)]
pub struct RuntimePostgresConfig {
    target: LambdaPostgresConfig,
    credentials: PostgresCredentials,
}

impl RuntimePostgresConfig {
    pub fn from_env() -> Result<Self, LambdaBootstrapConfigError> {
        let target = LambdaPostgresConfig::from_env()?;
        let credentials = PostgresCredentials::new(
            required_env("POSTGRES_USERNAME")?,
            required_env("POSTGRES_PASSWORD")?,
        )
        .map_err(|_| LambdaBootstrapConfigError::InvalidPostgres)?;
        Ok(Self {
            target,
            credentials,
        })
    }

    fn pool_config(&self) -> Result<PostgresPoolConfig, LambdaBootstrapConfigError> {
        PostgresPoolConfig::lambda(
            self.target.host.clone(),
            self.target.port,
            self.target.database.clone(),
            self.credentials.username().to_owned(),
            self.credentials.password().to_owned(),
            self.target.max_connections,
            self.target.root_certificate.clone(),
        )
        .map_err(|_| LambdaBootstrapConfigError::InvalidPostgres)
    }

    /// Creates the runtime pool. Invalid configuration fails with a static error; a pool failure
    /// is also recorded once as a setup diagnostic. The pool connects lazily, so a refused login
    /// surfaces on first use, in the failing work's own diagnostic.
    pub async fn connect(
        &self,
        component: &'static str,
    ) -> Result<sqlx::PgPool, lambda_runtime::Error> {
        self.pool_config()
            .map_err(|_| lambda_runtime::Error::from("invalid PostgreSQL configuration"))?
            .connect()
            .await
            .map_err(|error| {
                setup_failed(
                    component,
                    FailureDiagnostic::postgres("pool_connect", &error),
                    "failed to create PostgreSQL pool",
                )
            })
    }
}

/// Builds one composition per execution environment on first use and shares it with every later
/// invocation. A failed build stores nothing, so the next invocation retries it.
pub struct LazyComposition<T> {
    value: OnceCell<T>,
}

impl<T: Clone> LazyComposition<T> {
    pub const fn new() -> Self {
        Self {
            value: OnceCell::const_new(),
        }
    }

    /// Concurrent first invocations wait for a single build.
    pub async fn get_or_try_build<E, F, Fut>(&self, build: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.value.get_or_try_init(build).await.cloned()
    }
}

impl<T: Clone> Default for LazyComposition<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Records a failed composition step with only its bounded labels and standardized codes, and
/// returns the static error the Lambda runtime reports for it.
fn setup_failed(
    component: &'static str,
    failure: FailureDiagnostic,
    message: &'static str,
) -> lambda_runtime::Error {
    record_setup_failure(component, failure);
    lambda_runtime::Error::from(message)
}

fn record_setup_failure(component: &'static str, failure: FailureDiagnostic) {
    tracing::warn!(
        event = "lambda.setup.failed",
        component,
        stage = failure.stage(),
        dependency = failure.dependency(),
        error_category = failure.category(),
        sqlstate = failure.sqlstate(),
    );
}

pub fn required_config_from_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    required_env(name)
}

pub fn log_cold_start(component: &'static str, initialization_started_at: Instant) {
    tracing::info!(
        event = "lambda.initialized",
        component,
        cold_start_duration_ms = initialization_started_at.elapsed().as_millis(),
    );
}

#[derive(Debug)]
pub struct LambdaInvocationBudget {
    started_at: Instant,
    available: Duration,
}

impl LambdaInvocationBudget {
    pub fn from_context(context: &Context, cap: Duration, response_headroom: Duration) -> Self {
        Self::from_context_at(
            context,
            epoch_millis(),
            Instant::now(),
            cap,
            response_headroom,
        )
    }

    pub fn remaining(&self) -> Duration {
        self.available.saturating_sub(self.started_at.elapsed())
    }

    fn from_context_at(
        context: &Context,
        now_epoch_ms: u64,
        started_at: Instant,
        cap: Duration,
        response_headroom: Duration,
    ) -> Self {
        Self {
            started_at,
            available: available_budget(context, now_epoch_ms, cap, response_headroom),
        }
    }
}

fn available_budget(
    context: &Context,
    now_epoch_ms: u64,
    cap: Duration,
    response_headroom: Duration,
) -> Duration {
    Duration::from_millis(context.deadline.saturating_sub(now_epoch_ms))
        .min(cap)
        .saturating_sub(response_headroom)
}

pub fn log_invocation_start(component: &'static str, context: &Context) {
    tracing::info!(
        event = "lambda.invocation.started",
        component,
        request_id = %context.request_id,
        remaining_budget_ms = remaining_budget_ms(context.deadline, epoch_millis()),
    );
}

fn remaining_budget_ms(deadline_epoch_ms: u64, now_epoch_ms: u64) -> u64 {
    deadline_epoch_ms.saturating_sub(now_epoch_ms)
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn required_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(LambdaBootstrapConfigError::Missing { name }),
        Err(env::VarError::NotUnicode(_)) => Err(LambdaBootstrapConfigError::Invalid { name }),
    }
}

fn optional_env<T>(name: &'static str, default: T) -> Result<T, LambdaBootstrapConfigError>
where
    T: std::str::FromStr,
{
    optional_env_from_result(name, env::var(name), default)
}

fn optional_env_from_result<T>(
    name: &'static str,
    value: Result<String, env::VarError>,
    default: T,
) -> Result<T, LambdaBootstrapConfigError>
where
    T: std::str::FromStr,
{
    match value {
        Ok(value) => value
            .parse()
            .map_err(|_| LambdaBootstrapConfigError::Invalid { name }),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(LambdaBootstrapConfigError::Invalid { name }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_postgres::VersionedPostgresCredentials;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn should_use_default_when_optional_configuration_is_absent() {
        let value = optional_env_from_result("POSTGRES_PORT", Err(env::VarError::NotPresent), 5432);

        assert_eq!(Ok(5432), value);
    }

    #[test]
    fn should_reject_invalid_optional_configuration_without_echoing_its_value() {
        let value = optional_env_from_result("POSTGRES_PORT", Ok(String::from("not-a-port")), 5432);

        assert_eq!(
            Err(LambdaBootstrapConfigError::Invalid {
                name: "POSTGRES_PORT"
            }),
            value
        );
    }

    #[test]
    fn should_redact_versioned_credentials() {
        let credentials = VersionedPostgresCredentials::new(
            "version-1".to_owned(),
            "aura_runtime".to_owned(),
            "very-secret-password".to_owned(),
        );
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(error) => panic!("expected credentials: {error}"),
        };
        let output = format!("{credentials:?}");

        assert!(output.contains("version-1"));
        assert!(output.contains("<redacted>"));
        assert!(!output.contains("aura_runtime"));
        assert!(!output.contains("very-secret-password"));
    }

    #[tokio::test]
    async fn should_build_once_for_concurrent_first_invocations_and_reuse_the_composition() {
        let composition = Arc::new(LazyComposition::<usize>::new());
        let builds = Arc::new(AtomicUsize::new(0));
        let build = |builds: Arc<AtomicUsize>| {
            move || async move {
                tokio::task::yield_now().await;
                Ok::<_, ()>(builds.fetch_add(1, Ordering::AcqRel) + 1)
            }
        };

        let (left, right) = tokio::join!(
            composition.get_or_try_build(build(Arc::clone(&builds))),
            composition.get_or_try_build(build(Arc::clone(&builds))),
        );
        let warm = composition
            .get_or_try_build(|| async { Ok::<_, ()>(99) })
            .await;

        assert_eq!((Ok(1), Ok(1), Ok(1)), (left, right, warm));
        assert_eq!(1, builds.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn should_retry_a_failed_build_on_the_next_invocation() {
        let composition = LazyComposition::<&'static str>::new();

        let failed = composition
            .get_or_try_build(|| async { Err::<&str, _>("provider unavailable") })
            .await;
        let retried = composition
            .get_or_try_build(|| async { Ok::<_, &str>("composed") })
            .await;

        assert_eq!(Err("provider unavailable"), failed);
        assert_eq!(Ok("composed"), retried);
    }

    #[test]
    fn should_read_deploy_time_runtime_credentials_and_reject_blank_ones() {
        for (password, expected) in [("fixture-password", "ok"), (" ", "missing")] {
            let mut command =
                std::process::Command::new(std::env::current_exe().expect("test executable"));
            command
                .args([
                    "--exact",
                    "tests::runtime_credentials_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env_clear()
                .envs([
                    ("POSTGRES_HOST", "localhost"),
                    ("POSTGRES_PORT", "9"),
                    ("POSTGRES_DATABASE", "fixture"),
                    ("POSTGRES_USERNAME", "fixture"),
                    ("POSTGRES_PASSWORD", password),
                    ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem"),
                    ("RUNTIME_CREDENTIALS_PROBE_EXPECTED", expected),
                ]);
            // Keep subprocess coverage in CI's per-process output directory.
            if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
                command.env("LLVM_PROFILE_FILE", profile);
            }
            let output = command.output().expect("probe process");
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .contains("test result: ok. 1 passed"),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[tokio::test]
    #[ignore = "invoked by the runtime credentials test in a clean subprocess"]
    async fn runtime_credentials_probe() {
        let config = RuntimePostgresConfig::from_env();
        match env::var("RUNTIME_CREDENTIALS_PROBE_EXPECTED").as_deref() {
            Ok("ok") => {
                let pool = config
                    .expect("runtime credentials from the environment")
                    .connect("runtime-credentials-probe")
                    .await
                    .expect("lazy pool");
                assert_eq!(0, pool.size());
            }
            _ => assert!(matches!(
                config,
                Err(LambdaBootstrapConfigError::Missing {
                    name: "POSTGRES_PASSWORD"
                })
            )),
        }
    }

    #[test]
    fn should_clamp_elapsed_invocation_deadline_to_zero() {
        assert_eq!(0, remaining_budget_ms(100, 101));
    }

    #[test]
    fn should_cap_controlled_context_deadline_before_reserving_response_headroom() {
        let context = context_with_deadline(6_000);

        let available = available_budget(
            &context,
            1_000,
            Duration::from_secs(3),
            Duration::from_millis(500),
        );

        assert_eq!(Duration::from_millis(2_500), available);
    }

    #[test]
    fn should_saturate_elapsed_deadline_and_response_headroom_to_zero() {
        let elapsed = available_budget(
            &context_with_deadline(1_000),
            1_001,
            Duration::from_secs(1),
            Duration::from_millis(1),
        );
        let headroom = available_budget(
            &context_with_deadline(2_000),
            1_000,
            Duration::from_secs(1),
            Duration::from_secs(2),
        );

        assert_eq!(Duration::ZERO, elapsed);
        assert_eq!(Duration::ZERO, headroom);
    }

    fn context_with_deadline(deadline: u64) -> Context {
        let mut context = Context::default();
        context.deadline = deadline;
        context
    }
}
