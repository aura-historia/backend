use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use search_filter_projection_lambda::{
    compose_projection_use_case, handler_with_invocation_budget, invocation_budget,
};
use search_filter_service::use_cases::ProjectSearchFilterChangeUseCase;
use std::{future::Future, sync::Arc, time::Instant};
use url::Url;

const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
const WORKER_STAGE_ENV: &str = "STAGE";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let open_search = open_search_client(OpenSearchConfig::from_env()?)?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start("search-filter-projection-lambda", initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let open_search = open_search.clone();
            let use_cases = Arc::clone(&use_cases);
            async move {
                log_invocation_start("search-filter-projection-lambda", &event.context);
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
                            Ok::<_, Error>(compose_projection_use_case(pool, open_search))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type ProjectionUseCase = Arc<dyn ProjectSearchFilterChangeUseCase>;
type ProjectionUseCaseLease = VersionedCompositionLease<ProjectionUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ProjectionUseCaseLease, Error>>,
{
    handle_sqs_invocation(
        event,
        "search-filter-projection-lambda",
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
}

struct OpenSearchConfig {
    endpoint: Url,
    basic_auth: Option<(String, String)>,
}

impl OpenSearchConfig {
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
        })
    }
}

fn open_search_client(config: OpenSearchConfig) -> Result<OpenSearch, Error> {
    let pool = SingleNodeConnectionPool::new(config.endpoint);
    let builder = TransportBuilder::new(pool);
    let transport = match config.basic_auth {
        Some((username, password)) => builder.auth(Credentials::Basic(username, password)),
        None => builder,
    }
    .build()
    .map_err(|_| Error::from("failed to configure OpenSearch client"))?;
    Ok(OpenSearch::new(transport))
}
