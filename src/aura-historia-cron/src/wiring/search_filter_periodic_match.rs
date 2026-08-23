use crate::scheduled_job::CronJob;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use large_language_model::{VertexAiConfig, VertexAiGemini};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_postgres::{
    PostgresConnectError, PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork,
};
use product_opensearch::OpenSearchProductSearchReader;
use product_postgres::{
    SqlxProductCurrentRevisionGuardFactory, SqlxProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_postgres::{
    SqlxExistingSearchFilterMatchReader, SqlxPeriodicSearchFilterCandidateReader,
    SqlxPeriodicSearchFilterMatchingRunLock, SqlxPeriodicSearchFilterProgressFactory,
    SqlxSearchFilterMatchWriterFactory,
};
use search_filter_service::use_cases::{
    PeriodicSearchFilterMatchingPolicy, RunPeriodicSearchFilterMatchingHandler,
    RunPeriodicSearchFilterMatchingUseCase,
};
use std::{
    num::{NonZeroU64, NonZeroUsize},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const MAX_HYBRID_SCAN_LIMIT: usize = 100;

pub async fn build_from_env() -> Result<(Arc<dyn CronJob>, String, Duration), WiringError> {
    let config = PeriodicMatchConfig::from_env()?;
    let pool = config
        .postgres
        .connect()
        .await
        .map_err(PostgresConnectError::Connect)
        .map_err(WiringError::Postgres)?;
    let client = opensearch_client(&config)?;
    let credentials = GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| WiringError::VertexCredentials {
            detail: error.to_string(),
        })?;
    let evaluator = VertexAiGemini::new(
        VertexAiConfig::new(
            config.vertex_project_id,
            config.vertex_location,
            config.vertex_model,
        ),
        credentials,
    )
    .map_err(WiringError::VertexClient)?;
    let handler: Arc<dyn RunPeriodicSearchFilterMatchingUseCase> = Arc::new(
        RunPeriodicSearchFilterMatchingHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxPeriodicSearchFilterMatchingRunLock::new(config.postgres),
            SqlxPeriodicSearchFilterCandidateReader::new(pool.clone()),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductSearchReader::new(client),
            SqlxExistingSearchFilterMatchReader::new(pool),
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
            evaluator,
            SqlxProductCurrentRevisionGuardFactory::new(),
            SqlxSearchFilterMatchWriterFactory,
            SqlxPeriodicSearchFilterProgressFactory,
            config.policy,
        )
        .map_err(WiringError::Handler)?,
    );
    Ok((
        Arc::new(crate::jobs::SearchFilterPeriodicMatchJob::new(handler)),
        config.schedule,
        config.max_run_duration,
    ))
}

struct PeriodicMatchConfig {
    postgres: PostgresPoolConfig,
    endpoint: url::Url,
    auth: Option<(String, String)>,
    vertex_project_id: String,
    vertex_location: String,
    vertex_model: String,
    schedule: String,
    max_run_duration: Duration,
    policy: PeriodicSearchFilterMatchingPolicy,
}
impl PeriodicMatchConfig {
    fn from_env() -> Result<Self, WiringError> {
        let stage = std::env::var("STAGE").ok();
        let filter_page_size = nonzero("PERIODIC_MATCH_FILTER_PAGE_SIZE", 100)?;
        let hybrid_scan_limit = nonzero("PERIODIC_MATCH_HYBRID_SCAN_LIMIT", 100)?;
        let evaluation_limit = nonzero("PERIODIC_MATCH_EVALUATION_LIMIT", 50)?;
        let llm_concurrency = nonzero("PERIODIC_MATCH_LLM_CONCURRENCY", 8)?;
        let max_attempts = nonzero("PERIODIC_MATCH_MAX_ATTEMPTS", 3)?;
        if hybrid_scan_limit.get() > MAX_HYBRID_SCAN_LIMIT
            || evaluation_limit > hybrid_scan_limit
            || max_attempts.get() > 10
        {
            return Err(WiringError::InvalidPolicy);
        }
        let endpoint_raw = required("OPENSEARCH_ENDPOINT_URL")?;
        let endpoint = url::Url::parse(&endpoint_raw).map_err(WiringError::OpenSearchUrl)?;
        let auth = if matches!(stage.as_deref(), Some("local" | "test" | "ephemeral")) {
            None
        } else {
            Some((
                required("OPENSEARCH_USERNAME")?,
                required("OPENSEARCH_PASSWORD")?,
            ))
        };
        let postgres = PostgresPoolConfig::new(
            required("POSTGRES_HOST")?,
            number("POSTGRES_PORT", 5432)? as u16,
            required("POSTGRES_DATABASE")?,
            required("POSTGRES_USERNAME")?,
            required("POSTGRES_PASSWORD")?,
            number("POSTGRES_MAX_CONNECTIONS", 2)? as u32,
        )
        .map_err(WiringError::PostgresConfig)?;
        Ok(Self {
            postgres,
            endpoint,
            auth,
            vertex_project_id: required("VERTEX_AI_PROJECT_ID")?,
            vertex_location: required("VERTEX_AI_LOCATION")?,
            vertex_model: required("VERTEX_AI_MODEL")?,
            schedule: optional("SEARCH_FILTER_PERIODIC_MATCH_CRON", "0 0 15 * * * *"),
            max_run_duration: positive_duration("PERIODIC_MATCH_MAX_RUN_SECONDS", 7200)?,
            policy: PeriodicSearchFilterMatchingPolicy {
                filter_page_size,
                hybrid_scan_limit,
                evaluation_limit,
                llm_concurrency,
                max_attempts,
                projection_lag: time::Duration::seconds(number(
                    "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
                    900,
                )? as i64),
                replay_overlap: time::Duration::seconds(number(
                    "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
                    7200,
                )? as i64),
            },
        })
    }
}
fn required(name: &'static str) -> Result<String, WiringError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(WiringError::MissingEnv { name })
}
fn optional(name: &'static str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}
fn number<T>(name: &'static str, default: T) -> Result<T, WiringError>
where
    T: FromStr + Copy,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    std::env::var(name)
        .ok()
        .map(|value| {
            value.parse().map_err(|source| WiringError::InvalidNumber {
                name,
                source: Box::new(source),
            })
        })
        .unwrap_or(Ok(default))
}
fn nonzero(name: &'static str, default: usize) -> Result<NonZeroUsize, WiringError> {
    NonZeroUsize::new(number(name, default)?).ok_or(WiringError::InvalidPolicy)
}

fn positive_duration(name: &'static str, default: u64) -> Result<Duration, WiringError> {
    let seconds = NonZeroU64::new(number(name, default)?).ok_or(WiringError::InvalidPolicy)?;
    Ok(Duration::from_secs(seconds.get()))
}
fn opensearch_client(config: &PeriodicMatchConfig) -> Result<OpenSearch, WiringError> {
    let pool = SingleNodeConnectionPool::new(config.endpoint.clone());
    let builder = TransportBuilder::new(pool);
    let builder = match &config.auth {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.to_owned(), password.to_owned()))
        }
        None => builder,
    };
    Ok(OpenSearch::new(builder.build().map_err(|error| {
        WiringError::OpenSearch {
            detail: error.to_string(),
        }
    })?))
}
#[derive(Debug, thiserror::Error)]
pub enum WiringError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid numeric environment variable {name}")]
    InvalidNumber {
        name: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("invalid periodic matching policy")]
    InvalidPolicy,
    #[error("invalid PostgreSQL configuration")]
    PostgresConfig(#[source] PostgresPoolConfigError),
    #[error("invalid OpenSearch endpoint")]
    OpenSearchUrl(#[source] url::ParseError),
    #[error("failed to connect to PostgreSQL")]
    Postgres(#[source] PostgresConnectError),
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexCredentials { detail: String },
    #[error("failed to build Vertex AI client")]
    VertexClient(#[source] reqwest::Error),
    #[error("failed to build periodic matching handler")]
    Handler(#[source] search_filter_service::use_cases::RunPeriodicSearchFilterMatchingError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_zero_periodic_match_max_run_seconds() {
        let result = positive_duration("PERIODIC_MATCH_MAX_RUN_SECONDS", 0);
        assert!(matches!(result, Err(WiringError::InvalidPolicy)));
    }
}
