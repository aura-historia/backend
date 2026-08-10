use std::sync::Arc;

use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue;
use aura_historia_worker::search_filter_percolator::consume_search_filter_percolator_queue;
use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::watchlist_notifications::consume_watchlist_notification_queue;
use aura_historia_worker::{
    QueueConfig, WorkerConfig, WorkerConfigError, WorkerRunError, WorkerRuntime,
    run_until_shutdown_with_runtime,
};
use common::postgres::{PostgresConnectError, SqlxUnitOfWork, connect_from_env};
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use notification_dynamodb::conditional_writer::ConditionalDynamoDbNotificationWriter;
use notification_service::use_cases::commands::create_notification::CreateNotificationHandler;
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use product_postgres::{
    SqlxProductSearchFilterMatchSourceReaderFactory,
    SqlxProductWatchlistNotificationSourceReaderFactory,
};
use product_service::use_cases::{
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsUseCase,
};
use search_filter_opensearch::{
    OpenSearchSearchFilterIndex, VertexAiProductMatchEvaluator, VertexAiProductMatchEvaluatorConfig,
};
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterIndexReader,
    SqlxSearchFilterMatchNotificationSourceReaderFactory, SqlxSearchFilterMatchWriterFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationUseCase,
    MatchProductEventHandler, MatchProductEventUseCase, ProjectSearchFilterChangeHandler,
    ProjectSearchFilterChangeUseCase,
};
use user_postgres::SqlxUserTierEntitlementsFactory;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const WORKER_SCOPE_ENV: &str = "AURA_HISTORIA_WORKER_SCOPE";
const DYNAMODB_TABLE_NAME_ENV: &str = "DYNAMODB_TABLE_NAME";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "aura-historia";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[tokio::main]
async fn main() -> Result<(), MainError> {
    common::logging::init_logging();
    let config = WorkerConfig::from_env()?;
    let pool = connect_from_env().await?;

    match WorkerScope::from_env()? {
        WorkerScope::SearchFilterProjection => run_search_filter_projection(config, pool).await,
        WorkerScope::SearchFilterPercolator => run_search_filter_percolator(config, pool).await,
        WorkerScope::SearchFilterMatchNotification => {
            run_search_filter_match_notifications(config, pool).await
        }
        WorkerScope::WatchlistNotification => run_watchlist_notifications(config, pool).await,
    }
}

async fn run_search_filter_projection(
    config: WorkerConfig,
    pool: sqlx::PgPool,
) -> Result<(), MainError> {
    let client = opensearch_client_from_env()?;
    let handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
        Arc::new(ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(pool),
            OpenSearchSearchFilterIndex::new(client),
        ));
    let (runtime, mut receivers) =
        WorkerRuntime::with_search_filter_projection_queue(QueueConfig::new(1024))?;
    let receiver =
        receivers
            .take(WorkerQueue::SearchFilterOpenSearch)
            .ok_or(MainError::MissingQueue {
                queue: "search filter OpenSearch",
            })?;
    let task = tokio::spawn(consume_search_filter_projection_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_percolator(
    config: WorkerConfig,
    pool: sqlx::PgPool,
) -> Result<(), MainError> {
    let client = opensearch_client_from_env()?;
    let evaluator = vertex_ai_product_match_evaluator()?;
    let handler: Arc<dyn MatchProductEventUseCase> = Arc::new(MatchProductEventHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        OpenSearchSearchFilterIndex::new(client),
        evaluator,
        SqlxActiveSearchFilterMatchCandidateReaderFactory,
        SqlxSearchFilterMatchWriterFactory,
    ));
    let (runtime, mut receivers) =
        WorkerRuntime::with_search_filter_percolator_queue(QueueConfig::new(1024))?;
    let receiver =
        receivers
            .take(WorkerQueue::SearchFilterPercolator)
            .ok_or(MainError::MissingQueue {
                queue: "search filter percolator",
            })?;
    let task = tokio::spawn(consume_search_filter_percolator_queue(
        receiver,
        handler,
        SqlxUnitOfWork::new(pool),
        SqlxProductSearchFilterMatchSourceReaderFactory::new(),
    ));
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_match_notifications(
    config: WorkerConfig,
    pool: sqlx::PgPool,
) -> Result<(), MainError> {
    let table = std::env::var(DYNAMODB_TABLE_NAME_ENV).map_err(|_| MainError::MissingEnv {
        name: DYNAMODB_TABLE_NAME_ENV,
    })?;
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
        Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
            CreateNotificationHandler::new(ConditionalDynamoDbNotificationWriter::new(
                aws_sdk_dynamodb::Client::new(&aws_config),
                table,
            )),
        ));
    let (runtime, mut receivers) =
        WorkerRuntime::with_search_filter_match_notification_queue(QueueConfig::new(1024))?;
    let receiver = receivers
        .take(WorkerQueue::SearchFilterMatchNotification)
        .ok_or(MainError::MissingQueue {
            queue: "search filter match notification",
        })?;
    let task = tokio::spawn(consume_search_filter_match_notification_queue(
        receiver,
        handler,
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterMatchNotificationSourceReaderFactory,
        SqlxUnitOfWork::new(pool),
        SqlxProductSearchFilterMatchSourceReaderFactory::new(),
    ));
    finish_runtime(config, runtime, task).await
}

async fn run_watchlist_notifications(
    config: WorkerConfig,
    pool: sqlx::PgPool,
) -> Result<(), MainError> {
    let table = std::env::var(DYNAMODB_TABLE_NAME_ENV).map_err(|_| MainError::MissingEnv {
        name: DYNAMODB_TABLE_NAME_ENV,
    })?;
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let notification_writer = ConditionalDynamoDbNotificationWriter::new(
        aws_sdk_dynamodb::Client::new(&aws_config),
        table,
    );
    let handler: Arc<dyn GenerateWatchlistNotificationsUseCase> =
        Arc::new(GenerateWatchlistNotificationsHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductWatchlistNotificationSourceReaderFactory::new(),
            SqlxWatchlistNotificationRecipientReaderFactory,
            CreateNotificationHandler::new(notification_writer),
        ));
    let (runtime, mut receivers) =
        WorkerRuntime::with_watchlist_notification_queue(QueueConfig::new(1024))?;
    let receiver =
        receivers
            .take(WorkerQueue::WatchlistNotification)
            .ok_or(MainError::MissingQueue {
                queue: "watchlist notification",
            })?;
    let task = tokio::spawn(consume_watchlist_notification_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn finish_runtime(
    config: WorkerConfig,
    runtime: WorkerRuntime,
    task: tokio::task::JoinHandle<()>,
) -> Result<(), MainError> {
    let result = run_until_shutdown_with_runtime(config, runtime, shutdown_signal()).await;
    task.abort();
    let _ = task.await;
    result?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum WorkerScope {
    SearchFilterProjection,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    WatchlistNotification,
}

impl WorkerScope {
    fn from_env() -> Result<Self, MainError> {
        match std::env::var(WORKER_SCOPE_ENV)
            .unwrap_or_else(|_| "search-filter-projection".to_owned())
            .as_str()
        {
            "search-filter-projection" => Ok(Self::SearchFilterProjection),
            "search-filter-percolator" => Ok(Self::SearchFilterPercolator),
            "search-filter-match-notification" => Ok(Self::SearchFilterMatchNotification),
            "watchlist-notification" => Ok(Self::WatchlistNotification),
            value => Err(MainError::InvalidScope {
                value: value.to_owned(),
            }),
        }
    }
}

fn vertex_ai_product_match_evaluator() -> Result<VertexAiProductMatchEvaluator, MainError> {
    let config = VertexAiProductMatchEvaluatorConfig::new(
        std::env::var(VERTEX_AI_PROJECT_ID_ENV)
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_PROJECT_ID.to_owned()),
        std::env::var(VERTEX_AI_LOCATION_ENV)
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_LOCATION.to_owned()),
    );
    let credentials = GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| MainError::VertexAiCredentials {
            detail: error.to_string(),
        })?;
    Ok(VertexAiProductMatchEvaluator::new(config, credentials))
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
    #[error("{queue} queue is not registered")]
    MissingQueue { queue: &'static str },
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid worker scope {value}")]
    InvalidScope { value: String },
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexAiCredentials { detail: String },
    #[error(transparent)]
    Run(#[from] WorkerRunError),
}
