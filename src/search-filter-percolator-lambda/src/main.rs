use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use large_language_model::{VertexAiConfig, VertexAiGemini};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease,
    google_application_default_credentials, log_cold_start, log_invocation_start,
    logging_config_from_env, materialize_google_application_credentials_from_env,
    required_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use search_filter_percolator_lambda::{
    compose_percolator_use_case, handler_with_invocation_budget, invocation_budget,
};
use search_filter_service::use_cases::MatchProductListingEventUseCase;
use std::{future::Future, sync::Arc, time::Instant};
use url::Url;

const COMPONENT: &str = "search-filter-percolator-lambda";
const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
const VERTEX_AI_MODEL_ENV: &str = "VERTEX_AI_MODEL";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
const WORKER_STAGE_ENV: &str = "STAGE";
fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    materialize_google_application_credentials_from_env()?;
    init(logging_config_from_env());
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::from("failed to build Lambda async runtime"))?
        .block_on(async_main(initialization_started_at))
}

async fn async_main(initialization_started_at: Instant) -> Result<(), Error> {
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
    handle_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
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
    let credentials = google_application_default_credentials()?;
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
