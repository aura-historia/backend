use crate::{Runtime, RuntimeError};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease,
    record_credential_refresh_failure,
};
use platform_postgres::PostgresCredentialsProvider;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::sync::Arc;

pub type RuntimeLease = VersionedCompositionLease<Runtime>;

/// Refreshes credentials before every invocation and shares one composition per active version.
/// Existing leases keep their pools alive across replacement; refresh failures never use a
/// cached composition as a fallback.
pub struct RotatingRuntime {
    /// The Lambda component named in setup diagnostics.
    component: &'static str,
    postgres: LambdaPostgresConfig,
    credentials: Arc<dyn PostgresCredentialsProvider>,
    compositions: VersionedCompositionCache<Runtime>,
}

impl RotatingRuntime {
    pub fn new(
        component: &'static str,
        postgres: LambdaPostgresConfig,
        credentials: Arc<dyn PostgresCredentialsProvider>,
    ) -> Self {
        Self {
            component,
            postgres,
            credentials,
            compositions: VersionedCompositionCache::new(),
        }
    }

    pub async fn from_env(component: &'static str) -> Result<Self, RuntimeError> {
        let postgres = LambdaPostgresConfig::from_env()?;
        let credentials = postgres_credentials_provider_from_env()
            .await
            .map_err(RuntimeError::CredentialsProvider)?;
        Ok(Self::new(component, postgres, credentials))
    }

    /// A refresh failure is recorded once as a setup diagnostic and still returned typed.
    pub async fn current(&self) -> Result<RuntimeLease, RuntimeError> {
        let credentials = self
            .credentials
            .current()
            .await
            .inspect_err(|error| record_credential_refresh_failure(self.component, *error))?;
        self.compositions
            .get_or_try_build(credentials.version_id(), || async {
                let pool = self
                    .postgres
                    .pool_config(credentials.credentials())?
                    .connect()
                    .await
                    .map_err(RuntimeError::Pool)?;
                Ok(Runtime::from_pool(pool))
            })
            .await
    }
}
