use std::sync::Arc;

use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::{
    QueueConfig, WorkerConfig, WorkerConfigError, WorkerRunError, WorkerRuntime,
    run_until_shutdown_with_runtime,
};
use common::postgres::{PostgresConnectError, connect_from_env};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::SqlxSearchFilterIndexReader;
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
};

#[tokio::main]
async fn main() -> Result<(), MainError> {
    common::logging::init_logging();
    let config = WorkerConfig::from_env()?;
    let pool = connect_from_env().await?;
    let client = opensearch_client_from_env()?;

    let projection_handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
        Arc::new(ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(pool),
            OpenSearchSearchFilterIndex::new(client),
        ));
    let (runtime, mut receivers) =
        WorkerRuntime::with_search_filter_projection_queue(QueueConfig::new(1024))?;
    let receiver = receivers
        .take(WorkerQueue::SearchFilterOpenSearch)
        .ok_or(MainError::MissingSearchFilterQueue)?;
    let projection_task = tokio::spawn(consume_search_filter_projection_queue(
        receiver,
        projection_handler,
    ));

    let result = run_until_shutdown_with_runtime(config, runtime, shutdown_signal()).await;
    projection_task.abort();
    let _ = projection_task.await;
    result?;
    Ok(())
}

fn opensearch_client_from_env() -> Result<OpenSearch, MainError> {
    let endpoint = std::env::var("OPENSEARCH_ENDPOINT_URL").map_err(|_| MainError::MissingEnv {
        name: "OPENSEARCH_ENDPOINT_URL",
    })?;
    let endpoint = url::Url::parse(&endpoint).map_err(|error| MainError::OpenSearch {
        detail: error.to_string(),
    })?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".to_owned());
    let transport = if stage == "ephemeral" {
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint)).build()
    } else {
        let username = std::env::var("OPENSEARCH_USERNAME").map_err(|_| MainError::MissingEnv {
            name: "OPENSEARCH_USERNAME",
        })?;
        let password = std::env::var("OPENSEARCH_PASSWORD").map_err(|_| MainError::MissingEnv {
            name: "OPENSEARCH_PASSWORD",
        })?;
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
            .auth(Credentials::Basic(username, password))
            .build()
    }
    .map_err(|error| MainError::OpenSearch {
        detail: error.to_string(),
    })?;
    Ok(OpenSearch::new(transport))
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error(transparent)]
    Config(#[from] WorkerConfigError),
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error(transparent)]
    QueueConfig(#[from] aura_historia_worker::QueueConfigError),
    #[error("search filter OpenSearch queue is not registered")]
    MissingSearchFilterQueue,
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error(transparent)]
    Run(#[from] WorkerRunError),
}
