use aura_historia_worker::{
    OPENSEARCH_ENDPOINT_URL_ENV, OPENSEARCH_PASSWORD_ENV, OPENSEARCH_USERNAME_ENV, WORKER_STAGE_ENV,
};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
    required_config_from_env,
};
use platform_observability::init;
use platform_postgres::SqlxUnitOfWork;
use product_listing_opensearch::OpenSearchProductListingSearchProjection;
use product_listing_opensearch_lambda::handler;
use product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory;
use product_listing_service::use_cases::{
    ProjectProductListingHandler, ProjectProductListingUseCase,
};
use std::{sync::Arc, time::Instant};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let pool = LambdaPostgresConfig::from_env()?
        .into_pool_config()
        .connect()
        .await
        .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
    let open_search = open_search_client(OpenSearchConfig::from_env()?)?;
    let use_case: Arc<dyn ProjectProductListingUseCase> =
        Arc::new(ProjectProductListingHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductListingSearchProjection::new(open_search),
        ));

    log_cold_start(
        "product-listing-opensearch-lambda",
        initialization_started_at,
    );
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let use_case = Arc::clone(&use_case);
            async move {
                log_invocation_start("product-listing-opensearch-lambda", &event.context);
                handler(event, use_case.as_ref()).await
            }
        },
    ))
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
