use aura_historia_worker::notification_delivery::consume_notification_delivery_queue;
use aura_historia_worker::product_embedding::consume_product_embedding_queue;
use aura_historia_worker::product_opensearch::consume_product_opensearch_queue;
use aura_historia_worker::product_translation::consume_product_translation_queue;
use aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue;
use aura_historia_worker::search_filter_percolator::consume_search_filter_percolator_queue;
use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::watchlist_notifications::consume_watchlist_notification_queue;
use aura_historia_worker::{
    QueueConfig, WorkerOpenSearchConfig, WorkerRunError, WorkerRuntimeComposition, WorkerScope,
    WorkerStartupConfig, WorkerStartupConfigError, WorkerVertexAiConfig,
    run_until_shutdown_with_runtime,
};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::Client as SesClient;
use aws_smithy_types::timeout::TimeoutConfig;
use embedding::{VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use large_language_model::{VertexAiConfig, VertexAiGemini};
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{
    SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryIntentRepositoryFactory,
    SqlxNotificationDeliveryRepository, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSender, NotificationDeliveryDispatcher,
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationUseCase,
    },
};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresConnectError, SqlxUnitOfWork};
use product_opensearch::OpenSearchProductSearchProjection;
use product_postgres::{
    SqlxProductCurrentRevisionGuardFactory, SqlxProductEmbeddingSourceReader,
    SqlxProductEmbeddingWriterFactory, SqlxProductSearchFilterMatchSourceReaderFactory,
    SqlxProductTranslationSourceReader, SqlxProductTranslationWriterFactory,
    SqlxProductWatchlistNotificationSourceReaderFactory,
};
use product_service::use_cases::{
    EmbedProductEventHandler, EmbedProductEventUseCase, GenerateWatchlistNotificationsHandler,
    GenerateWatchlistNotificationsUseCase, ProjectProductHandler, ProjectProductUseCase,
    TranslateProductEventHandler, TranslateProductEventUseCase,
};
use product_translation_llm::LargeLanguageModelProductTitleTranslator;
use search_filter_opensearch::OpenSearchSearchFilterIndex;
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
use std::sync::Arc;
use user_postgres::SqlxUserTierEntitlementsFactory;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[tokio::main]
async fn main() -> Result<(), MainError> {
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));
    let startup = WorkerStartupConfig::from_env()?;
    let scope = startup.scope();
    let worker_config = startup.worker().clone();
    let pool = startup
        .postgres()
        .connect()
        .await
        .map_err(PostgresConnectError::Connect)?;
    let composition = WorkerRuntimeComposition::build(scope, QueueConfig::new(1024))?;

    match scope {
        WorkerScope::SearchFilterProjection => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_search_filter_projection(worker_config, pool, composition, opensearch).await
        }
        WorkerScope::SearchFilterPercolator => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_search_filter_percolator(worker_config, pool, composition, opensearch, vertex_ai)
                .await
        }
        WorkerScope::SearchFilterMatchNotification => {
            run_search_filter_match_notifications(worker_config, pool, composition).await
        }
        WorkerScope::WatchlistNotification => {
            run_watchlist_notifications(worker_config, pool, composition).await
        }
        WorkerScope::ProductTranslation => {
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_translation(worker_config, pool, composition, vertex_ai).await
        }
        WorkerScope::ProductOpenSearch => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_opensearch(worker_config, pool, composition, opensearch).await
        }
        WorkerScope::ProductEmbedding => {
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_embedding(worker_config, pool, composition, vertex_ai).await
        }
        WorkerScope::NotificationDelivery => {
            let delivery = startup
                .notification_delivery()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_notification_delivery(worker_config, pool, composition, delivery).await
        }
    }
}

async fn run_search_filter_projection(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
        Arc::new(ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(pool),
            OpenSearchSearchFilterIndex::new(opensearch_client(opensearch)?),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_search_filter_projection_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_percolator(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn MatchProductEventUseCase> = Arc::new(MatchProductEventHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductSearchFilterMatchSourceReaderFactory::new(),
        SqlxProductCurrentRevisionGuardFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchSearchFilterIndex::new(opensearch_client(opensearch)?),
        vertex_ai_large_language_model(vertex_ai)?,
        SqlxActiveSearchFilterMatchCandidateReaderFactory,
        SqlxSearchFilterMatchWriterFactory,
    ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_search_filter_percolator_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_match_notifications(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
        Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxSearchFilterMatchNotificationSourceReaderFactory,
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
            SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
            SqlxProductCurrentRevisionGuardFactory::new(),
            NotificationCreationCoordinatorFactory::new(
                SqlxNotificationRepositoryFactory::new(),
                InitialExternalDeliveryPlanReaderFactory,
                SqlxNotificationDeliveryIntentRepositoryFactory::new(),
            ),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_search_filter_match_notification_queue(
        receiver, handler,
    ));
    finish_runtime(config, runtime, task).await
}

async fn run_product_opensearch(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn ProjectProductUseCase> = Arc::new(ProjectProductHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductSearchFilterMatchSourceReaderFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchProductSearchProjection::new(opensearch_client(opensearch)?),
    ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_product_opensearch_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_product_embedding(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn EmbedProductEventUseCase> = Arc::new(EmbedProductEventHandler::new(
        SqlxProductEmbeddingSourceReader::new(pool.clone()),
        VertexAiEmbeddingGenerator::new(
            VertexAiEmbeddingConfig::new(vertex_ai.project_id(), vertex_ai.location()),
            vertex_ai_credentials()?,
        ),
        SqlxUnitOfWork::new(pool),
        SqlxProductEmbeddingWriterFactory::new(),
    ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_product_embedding_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_product_translation(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn TranslateProductEventUseCase> =
        Arc::new(TranslateProductEventHandler::new(
            SqlxProductTranslationSourceReader::new(pool.clone()),
            LargeLanguageModelProductTitleTranslator::new(vertex_ai_large_language_model(
                vertex_ai,
            )?),
            SqlxUnitOfWork::new(pool),
            SqlxProductTranslationWriterFactory::new(),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_product_translation_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_notification_delivery(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    delivery: &aura_historia_worker::WorkerNotificationDeliveryConfig,
) -> Result<(), MainError> {
    let email = delivery.email().ok_or(MainError::MissingScopeConfig {
        scope: WorkerScope::NotificationDelivery,
    })?;
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .operation_attempt_timeout(std::time::Duration::from_secs(20))
                .operation_timeout(std::time::Duration::from_secs(30))
                .build(),
        )
        .load()
        .await;
    let dispatcher =
        NotificationDeliveryDispatcher::new(vec![Arc::new(SesNotificationChannelSender::new(
            S3Client::new(&aws_config),
            SesClient::new(&aws_config),
            EmailDeliveryConfig::new(
                email.template_bucket(),
                email.from_email_address(),
                email.reply_to_email_address(),
                email.stage(),
                email.commit_sha(),
            ),
            Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
        )) as Arc<dyn NotificationChannelSender>])
        .map_err(MainError::NotificationDispatcher)?;
    dispatcher
        .validate_channels([NotificationDeliveryChannel::Email])
        .map_err(MainError::NotificationDispatch)?;
    let handler: Arc<dyn DeliverNotificationUseCase> = Arc::new(DeliverNotificationHandler::new(
        SqlxNotificationDeliveryRepository::new(pool),
        dispatcher,
    ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_notification_delivery_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn run_watchlist_notifications(
    config: aura_historia_worker::WorkerConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn GenerateWatchlistNotificationsUseCase> =
        Arc::new(GenerateWatchlistNotificationsHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductWatchlistNotificationSourceReaderFactory::new(),
            SqlxWatchlistNotificationRecipientReaderFactory,
            SqlxProductCurrentRevisionGuardFactory::new(),
            NotificationCreationCoordinatorFactory::new(
                SqlxNotificationRepositoryFactory::new(),
                InitialExternalDeliveryPlanReaderFactory,
                SqlxNotificationDeliveryIntentRepositoryFactory::new(),
            ),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = tokio::spawn(consume_watchlist_notification_queue(receiver, handler));
    finish_runtime(config, runtime, task).await
}

async fn finish_runtime(
    config: aura_historia_worker::WorkerConfig,
    runtime: aura_historia_worker::WorkerRuntime,
    task: tokio::task::JoinHandle<()>,
) -> Result<(), MainError> {
    let result = run_until_shutdown_with_runtime(config, runtime, shutdown_signal()).await;
    task.abort();
    let _ = task.await;
    result?;
    Ok(())
}

fn vertex_ai_large_language_model(
    config: &WorkerVertexAiConfig,
) -> Result<VertexAiGemini, MainError> {
    let config = VertexAiConfig::new(
        config.project_id().to_owned(),
        config.location().to_owned(),
        config
            .model()
            .ok_or(MainError::MissingVertexAiModel)?
            .to_owned(),
    );
    let credentials = vertex_ai_credentials()?;
    VertexAiGemini::new(config, credentials).map_err(MainError::VertexAiHttpClient)
}

fn vertex_ai_credentials()
-> Result<google_cloud_auth::credentials::AccessTokenCredentials, MainError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| MainError::VertexAiCredentials {
            detail: error.to_string(),
        })
}

fn opensearch_client(config: &WorkerOpenSearchConfig) -> Result<OpenSearch, MainError> {
    let pool = SingleNodeConnectionPool::new(config.endpoint().clone());
    let builder = TransportBuilder::new(pool);
    let transport = match config.basic_auth() {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.to_owned(), password.to_owned()))
        }
        None => builder,
    }
    .build()
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
    StartupConfig(#[from] WorkerStartupConfigError),
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error(transparent)]
    QueueConfig(#[from] aura_historia_worker::QueueConfigError),
    #[error("missing validated configuration for {scope:?} worker scope")]
    MissingScopeConfig { scope: WorkerScope },

    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexAiCredentials { detail: String },
    #[error("failed to build Vertex AI HTTP client: {0}")]
    VertexAiHttpClient(reqwest::Error),
    #[error("validated Vertex AI LLM configuration is missing its model")]
    MissingVertexAiModel,
    #[error(transparent)]
    NotificationDispatcher(
        #[from] notification_service::ports::notification_channel_sender::NotificationDeliveryDispatcherRegistrationError,
    ),
    #[error(transparent)]
    NotificationDispatch(
        #[from] notification_service::ports::notification_channel_sender::NotificationDeliveryDispatchError,
    ),
    #[error(transparent)]
    Run(#[from] WorkerRunError),
}
