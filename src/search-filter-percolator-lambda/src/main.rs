use aura_historia_worker::{
    OPENSEARCH_ENDPOINT_URL_ENV, OPENSEARCH_PASSWORD_ENV, OPENSEARCH_USERNAME_ENV,
    VERTEX_AI_LOCATION_ENV, VERTEX_AI_MODEL_ENV, VERTEX_AI_PROJECT_ID_ENV, WORKER_STAGE_ENV,
};
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use large_language_model::{VertexAiConfig, VertexAiGemini};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use search_filter_percolator_lambda::{
    compose_percolator_use_case, handler_with_invocation_budget, invocation_budget,
    retain_all_records,
};
use search_filter_service::use_cases::MatchProductListingEventUseCase;
use std::{
    fs::{self, OpenOptions},
    future::Future,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tracing::warn;
use url::Url;

const COMPONENT: &str = "search-filter-percolator-lambda";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const GOOGLE_ADC_CREDENTIALS_JSON_ENV: &str = "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_ADC_CREDENTIALS_DIRECTORY: &str = "/tmp/aura-historia-google-adc";
const GOOGLE_ADC_CREDENTIALS_FILE_NAME: &str = "application_default_credentials.json";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    materialize_google_application_credentials_from_env()?;
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let config = PercolatorConfig::from_env()?;
    let open_search = open_search_client(&config)?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let open_search = open_search.clone();
            let config = config.clone();
            let use_cases = Arc::clone(&use_cases);
            async move {
                log_invocation_start(COMPONENT, &event.context);
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
                            let evaluator = vertex_ai_evaluator(&config)?;
                            Ok::<_, Error>(compose_percolator_use_case(
                                pool,
                                open_search,
                                evaluator,
                            ))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type PercolatorUseCase = Arc<dyn MatchProductListingEventUseCase>;
type PercolatorUseCaseLease = VersionedCompositionLease<PercolatorUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<PercolatorUseCaseLease, Error>>,
{
    let budget = invocation_budget(&event.context);
    let Some(use_case) = complete_before_invocation_deadline(&budget, setup()).await else {
        warn!(
            outcome = "invocation_setup_timeout",
            "Saved-filter percolator setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    let Ok(use_case) = use_case else {
        warn!(
            outcome = "invocation_setup_failed",
            "Saved-filter percolator setup retained every record for SQS retry or redrive"
        );
        return retain_all_records(&event);
    };
    handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
}

async fn complete_before_invocation_deadline<T>(
    budget: &platform_lambda_bootstrap::LambdaInvocationBudget,
    operation: impl Future<Output = T>,
) -> Option<T> {
    let remaining = budget.remaining();
    if remaining.is_zero() {
        return None;
    }
    tokio::time::timeout(remaining, operation).await.ok()
}

#[derive(Clone)]
struct PercolatorConfig {
    endpoint: Url,
    basic_auth: Option<(String, String)>,
    vertex_project_id: String,
    vertex_location: String,
    vertex_model: String,
}

impl PercolatorConfig {
    fn from_env() -> Result<Self, Error> {
        let stage = required_config_from_env(WORKER_STAGE_ENV)?;
        let endpoint = required_config_from_env(OPENSEARCH_ENDPOINT_URL_ENV)?;
        let endpoint =
            Url::parse(&endpoint).map_err(|_| Error::from("invalid OpenSearch endpoint"))?;
        let basic_auth = if stage == "ephemeral" {
            None
        } else {
            Some((
                required_config_from_env(OPENSEARCH_USERNAME_ENV)?,
                required_config_from_env(OPENSEARCH_PASSWORD_ENV)?,
            ))
        };
        Ok(Self {
            endpoint,
            basic_auth,
            vertex_project_id: required_config_from_env(VERTEX_AI_PROJECT_ID_ENV)?,
            vertex_location: required_config_from_env(VERTEX_AI_LOCATION_ENV)?,
            vertex_model: required_config_from_env(VERTEX_AI_MODEL_ENV)?,
        })
    }
}

fn open_search_client(config: &PercolatorConfig) -> Result<OpenSearch, Error> {
    let pool = SingleNodeConnectionPool::new(config.endpoint.clone());
    let builder = TransportBuilder::new(pool);
    let transport = match &config.basic_auth {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.clone(), password.clone()))
        }
        None => builder,
    }
    .build()
    .map_err(|_| Error::from("failed to configure OpenSearch client"))?;
    Ok(OpenSearch::new(transport))
}

fn vertex_ai_evaluator(config: &PercolatorConfig) -> Result<VertexAiGemini, Error> {
    let credentials = GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|_| Error::from("failed to configure Google application credentials"))?;
    VertexAiGemini::new(
        VertexAiConfig::new(
            config.vertex_project_id.clone(),
            config.vertex_location.clone(),
            config.vertex_model.clone(),
        ),
        credentials,
    )
    .map_err(|_| Error::from("failed to configure Vertex AI client"))
}

fn materialize_google_application_credentials_from_env() -> Result<(), Error> {
    let credentials_json = std::env::var(GOOGLE_ADC_CREDENTIALS_JSON_ENV)
        .map_err(|_| Error::from("Google application credentials are unavailable"))?;
    let credentials_path = materialize_google_application_credentials(
        &credentials_json,
        Path::new(GOOGLE_ADC_CREDENTIALS_DIRECTORY),
    )?;
    // SAFETY: this runs in synchronous initialization before Lambda starts worker threads.
    unsafe {
        std::env::set_var(GOOGLE_APPLICATION_CREDENTIALS_ENV, credentials_path);
        std::env::remove_var(GOOGLE_ADC_CREDENTIALS_JSON_ENV);
    }
    Ok(())
}

fn materialize_google_application_credentials(
    credentials_json: &str,
    credentials_directory: &Path,
) -> Result<PathBuf, Error> {
    if !serde_json::from_str::<serde_json::Value>(credentials_json)
        .map(|value| value.is_object())
        .unwrap_or(false)
    {
        return Err(Error::from("invalid Google application credentials"));
    }
    fs::create_dir_all(credentials_directory)
        .map_err(|_| Error::from("failed to prepare Google credential directory"))?;
    fs::set_permissions(credentials_directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| Error::from("failed to protect Google credential directory"))?;
    let credentials_path = credentials_directory.join(GOOGLE_ADC_CREDENTIALS_FILE_NAME);
    let mut credentials_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&credentials_path)
        .map_err(|_| Error::from("failed to create Google credentials file"))?;
    credentials_file
        .write_all(credentials_json.as_bytes())
        .map_err(|_| Error::from("failed to write Google credentials file"))?;
    credentials_file
        .sync_all()
        .map_err(|_| Error::from("failed to synchronize Google credentials file"))?;
    Ok(credentials_path)
}
