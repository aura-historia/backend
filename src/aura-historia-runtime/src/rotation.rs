use crate::{Runtime, RuntimeError};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease,
};
use platform_postgres::PostgresCredentialsProvider;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use std::sync::Arc;

pub type RuntimeLease = VersionedCompositionLease<Runtime>;

/// Refreshes credentials before every invocation and shares one composition per active version.
/// Existing leases keep their pools alive across replacement; refresh failures never use a
/// cached composition as a fallback.
pub struct RotatingRuntime {
    postgres: LambdaPostgresConfig,
    credentials: Arc<dyn PostgresCredentialsProvider>,
    compositions: VersionedCompositionCache<Runtime>,
}

impl RotatingRuntime {
    pub fn new(
        postgres: LambdaPostgresConfig,
        credentials: Arc<dyn PostgresCredentialsProvider>,
    ) -> Self {
        Self {
            postgres,
            credentials,
            compositions: VersionedCompositionCache::new(),
        }
    }

    pub async fn from_env() -> Result<Self, RuntimeError> {
        let postgres = LambdaPostgresConfig::from_env()?;
        let credentials = postgres_credentials_provider_from_env()
            .await
            .map_err(RuntimeError::CredentialsProvider)?;
        Ok(Self::new(postgres, credentials))
    }

    pub async fn current(&self) -> Result<RuntimeLease, RuntimeError> {
        let credentials = self.credentials.current().await?;
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
