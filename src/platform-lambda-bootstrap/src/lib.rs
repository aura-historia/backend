use lambda_runtime::Context;
use platform_observability::{LogLevel, LoggingConfig};
use platform_postgres::{PostgresCredentials, PostgresPoolConfig};
use std::{
    env,
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::sync::Mutex;

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

    pub fn pool_config(
        &self,
        credentials: &PostgresCredentials,
    ) -> Result<PostgresPoolConfig, LambdaBootstrapConfigError> {
        PostgresPoolConfig::lambda(
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

/// A cloned lease keeps its composed pool and handlers alive until its invocation completes.
#[derive(Clone)]
pub struct VersionedCompositionLease<T> {
    version_id: String,
    value: Arc<T>,
}

impl<T> VersionedCompositionLease<T> {
    pub fn version_id(&self) -> &str {
        &self.version_id
    }

    pub fn value(&self) -> &T {
        self.value.as_ref()
    }
}

struct CachedComposition<T> {
    version_id: String,
    value: Arc<T>,
}

/// Holds only the active composition. Replacing it never explicitly closes an old pool.
pub struct VersionedCompositionCache<T> {
    current: Mutex<Option<CachedComposition<T>>>,
}

impl<T> VersionedCompositionCache<T> {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }

    /// Serializes a version change so concurrent invocations build one composition for one version.
    pub async fn get_or_try_build<E, F, Fut>(
        &self,
        version_id: &str,
        build: F,
    ) -> Result<VersionedCompositionLease<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut current = self.current.lock().await;
        if let Some(cached) = current.as_ref()
            && cached.version_id == version_id
        {
            return Ok(VersionedCompositionLease {
                version_id: cached.version_id.clone(),
                value: Arc::clone(&cached.value),
            });
        }

        let value = Arc::new(build().await?);
        let lease = VersionedCompositionLease {
            version_id: version_id.to_owned(),
            value: Arc::clone(&value),
        };
        *current = Some(CachedComposition {
            version_id: version_id.to_owned(),
            value,
        });
        Ok(lease)
    }
}

impl<T> Default for VersionedCompositionCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn required_config_from_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    required_env(name)
}

pub fn logging_config_from_env() -> LoggingConfig {
    let level = env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    async fn should_reuse_same_version_and_build_changed_version_once_concurrently() {
        let cache = Arc::new(VersionedCompositionCache::<usize>::new());
        let builds = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_try_build("version-1", {
                let builds = Arc::clone(&builds);
                move || async move { Ok::<_, ()>(builds.fetch_add(1, Ordering::AcqRel) + 1) }
            })
            .await;
        let first = match first {
            Ok(lease) => lease,
            Err(()) => panic!("first composition failed"),
        };
        let same = cache
            .get_or_try_build("version-1", || async { Ok::<_, ()>(99) })
            .await;
        let same = match same {
            Ok(lease) => lease,
            Err(()) => panic!("same-version composition failed"),
        };

        let left_cache = Arc::clone(&cache);
        let left_builds = Arc::clone(&builds);
        let right_cache = Arc::clone(&cache);
        let right_builds = Arc::clone(&builds);
        let (left, right) = tokio::join!(
            left_cache.get_or_try_build("version-2", move || async move {
                tokio::task::yield_now().await;
                Ok::<_, ()>(left_builds.fetch_add(1, Ordering::AcqRel) + 1)
            }),
            right_cache.get_or_try_build("version-2", move || async move {
                Ok::<_, ()>(right_builds.fetch_add(1, Ordering::AcqRel) + 1)
            }),
        );
        let left = match left {
            Ok(lease) => lease,
            Err(()) => panic!("left refreshed composition failed"),
        };
        let right = match right {
            Ok(lease) => lease,
            Err(()) => panic!("right refreshed composition failed"),
        };

        assert_eq!(first.version_id(), "version-1");
        assert_eq!(*first.value(), 1);
        assert_eq!(same.version_id(), "version-1");
        assert_eq!(*same.value(), 1);
        assert_eq!(left.version_id(), "version-2");
        assert_eq!(right.version_id(), "version-2");
        assert_eq!(*left.value(), *right.value());
        assert_eq!(builds.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn should_keep_old_composition_lease_alive_after_version_replacement() {
        let cache = VersionedCompositionCache::<String>::new();
        let old = cache
            .get_or_try_build("version-1", || async { Ok::<_, ()>("old-pool".to_owned()) })
            .await;
        let old = match old {
            Ok(lease) => lease,
            Err(()) => panic!("old composition failed"),
        };
        let new = cache
            .get_or_try_build("version-2", || async { Ok::<_, ()>("new-pool".to_owned()) })
            .await;
        let new = match new {
            Ok(lease) => lease,
            Err(()) => panic!("new composition failed"),
        };

        assert_eq!(old.version_id(), "version-1");
        assert_eq!(old.value(), "old-pool");
        assert_eq!(new.version_id(), "version-2");
        assert_eq!(new.value(), "new-pool");
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
