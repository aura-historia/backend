//! Production server binary for the crawler.
//!
//! Sets up logging with optional CloudWatch export, crawler-local Postgres, and authoritative
//! business Postgres, then hands the production profile to [`crawler::runtime`]. The runtime
//! continuously spiders ListingSource websites, scrapes product pages, and captures changed raw
//! ProductListing observations for asynchronous normalization. SIGTERM and SIGINT stop it
//! gracefully; see `docs/crawler/operations.md#stopping-the-crawler`.
//!
//! # Connection pool sizing
//!
//! `db_max_connections` defaults to
//! `spider_concurrency + scraper_concurrency + 10`.
//! Override it explicitly in [`CrawlerCronConfig`] if needed.
//!
//! # Environment variables
//!
//! | Variable                        | Purpose                                                        |
//! |---------------------------------|----------------------------------------------------------------|
//! | `LOCAL_DB_URL`                  | Required crawler-local Postgres connection URL, e.g. `postgres://crawler:<password>@127.0.0.1:5432/crawler_server`. The operator creates the database; `server` only applies crawler-local migrations to it. |
//! | `BUSINESS_DATABASE_URL`         | Required authoritative Postgres URL for listing_sources and products |
//! | `SPIDER_MAX_SIZE_BYTES`         | Required Spider transport page-body ceiling; set to `8388608`  |
//! | `CRAWLER_AUTHORING_MODE`        | `auto`, `review`, `manual` or `agent`; unset or blank means `review`. `agent` needs `CRAWLER_REVIEW_AUTH_TOKEN` and writes the review agents' tokens to `review-agent-tokens.json` in the working directory |
//! | `VERTEX_AI_PROJECT_ID`          | Google Cloud project for Vertex AI; required unless `CRAWLER_AUTHORING_MODE` is `manual` or `agent` |
//! | `VERTEX_AI_LOCATION`            | Vertex AI location; required unless `CRAWLER_AUTHORING_MODE` is `manual` or `agent` |
//! | `GOOGLE_APPLICATION_CREDENTIALS`| Optional local Application Default Credentials file; unless the mode is `manual` or `agent`, startup fails when Application Default Credentials cannot be loaded |
//! | `VERTEX_AI_MODEL`               | Schema generation/repair model (default: `gemini-3.5-flash`) |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default model for low-risk crawler LLM tasks                   |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override       |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls (default: `1`)          |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between crawler LLM request starts (default: `2000`) |
//! | `CRAWLER_CLOUDWATCH_LOG_GROUP`  | Optional CloudWatch Logs group name for crawler server logs    |
//! | `CRAWLER_CLOUDWATCH_LOG_STREAM` | Optional CloudWatch Logs stream name; defaults to host name    |
//!
//! With `CRAWLER_AUTHORING_MODE` `manual` or `agent` no Vertex AI or LLM setting is read
//! ([`CrawlerRuntimeConfig::from_env`]). The review console settings are in
//! `docs/crawler/operations.md#review-service`.
//!
//! # CloudWatch IAM permissions
//!
//! If `CRAWLER_CLOUDWATCH_LOG_GROUP` is set, the crawler runtime needs:
//!
//! - `logs:CreateLogGroup`
//! - `logs:CreateLogStream`
//! - `logs:PutLogEvents`

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudwatchlogs::Client as CloudWatchLogsClient;
use aws_sdk_cloudwatchlogs::error::SdkError;
use aws_sdk_cloudwatchlogs::operation::create_log_group::CreateLogGroupError;
use aws_sdk_cloudwatchlogs::operation::create_log_stream::CreateLogStreamError;
use crawler::local_db::local_db_url_from_env;
use crawler::logging::{
    CloudWatchBootstrapClient, CloudWatchBootstrapError, CloudWatchLoggingConfig,
    HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE, cloudwatch_logging_config,
    ensure_cloudwatch_log_destination,
};
use crawler::metrics::{EMF_LOG_DIRECTIVE, EmfPassthroughFormat};
use crawler::review::server::ReviewServerConfig;
use crawler::runtime::{CrawlerRuntimeConfig, CrawlerRuntimeProfile, apply_local_migrations};
use crawler::scraper::scraper_service::DEFAULT_SCHEMA_SEED_PAGES;
use crawler::service::cron::CrawlerCronConfig;
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationSource, ListingSourceSyncError, RegisteredListingSource,
};
use crawler::service::raw_capture::ProductListingRawCaptureServiceImpl;
use listing_source_postgres::SqlxListingSourceReaders;
use listing_source_service::ports::WebCrawlSourceReader;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ---------------------------------------------------------------------------
// ListingSourceRegistrationSource backed only by WebCrawlSourceReader.
// ---------------------------------------------------------------------------

struct PostgresWebCrawlSource {
    sources: Box<dyn WebCrawlSourceReader>,
}

impl PostgresWebCrawlSource {
    fn new(sources: Box<dyn WebCrawlSourceReader>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl ListingSourceRegistrationSource for PostgresWebCrawlSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        self.sources
            .list_sources()
            .await
            .map_err(|error| ListingSourceSyncError::FetchError(error.to_string()))?
            .into_iter()
            .map(|source| {
                Ok(RegisteredListingSource {
                    listing_source_id: source.listing_source_id,
                    listing_source_name: source.listing_source_name,
                    listing_source_slug: source.listing_source_slug,
                    crawl_enabled: source.web_crawl_enabled,
                    fallback_currency: source.fallback_currency,
                })
            })
            .collect()
    }
}

struct AwsSdkCloudWatchBootstrapClient {
    client: CloudWatchLogsClient,
}

#[async_trait]
impl CloudWatchBootstrapClient for AwsSdkCloudWatchBootstrapClient {
    async fn create_log_group(&self, log_group_name: &str) -> Result<(), CloudWatchBootstrapError> {
        match self
            .client
            .create_log_group()
            .log_group_name(log_group_name)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => Err(map_create_log_group_error(err.err())),
            Err(err) => Err(CloudWatchBootstrapError::Other(err.to_string())),
        }
    }

    async fn create_log_stream(
        &self,
        log_group_name: &str,
        log_stream_name: &str,
    ) -> Result<(), CloudWatchBootstrapError> {
        match self
            .client
            .create_log_stream()
            .log_group_name(log_group_name)
            .log_stream_name(log_stream_name)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => Err(map_create_log_stream_error(err.err())),
            Err(err) => Err(CloudWatchBootstrapError::Other(err.to_string())),
        }
    }
}

fn map_create_log_group_error(error: &CreateLogGroupError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(error.to_string())
    }
}

fn map_create_log_stream_error(error: &CreateLogStreamError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(error.to_string())
    }
}

fn build_log_filter() -> EnvFilter {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let raw_level = configured_log_level
        .as_deref()
        .unwrap_or("info")
        .to_string();
    let directives = format!(
        "{raw_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE},{EMF_LOG_DIRECTIVE}"
    );
    EnvFilter::new(directives)
}

fn init_crawler_logging(
    cloudwatch_config: Option<&CloudWatchLoggingConfig>,
    cloudwatch_client: Option<CloudWatchLogsClient>,
) -> Option<tracing_cloudwatch::CloudWatchWorkerGuard> {
    // Both sinks write metric records as bare EMF objects; see `crawler::metrics`.
    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .event_format(EmfPassthroughFormat::new(
            tracing_subscriber::fmt::format()
                .json()
                .with_current_span(true)
                .with_span_list(true),
        ));

    if let (Some(config), Some(client)) = (cloudwatch_config, cloudwatch_client) {
        let (cloudwatch_layer, cloudwatch_guard) = tracing_cloudwatch::layer()
            .with_fmt_layer(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .event_format(EmfPassthroughFormat::new(
                        tracing_subscriber::fmt::format()
                            .json()
                            .with_current_span(true)
                            .with_span_list(true)
                            .with_file(false)
                            .with_line_number(false)
                            .with_target(false),
                    )),
            )
            .with_client(
                client,
                tracing_cloudwatch::ExportConfig::default()
                    .with_batch_size(50usize)
                    .with_interval(Duration::from_secs(1))
                    .with_log_group_name(config.log_group_name.clone())
                    .with_log_stream_name(config.log_stream_name.clone()),
            );

        tracing_subscriber::registry()
            .with(build_log_filter())
            .with(stdout_layer)
            .with(cloudwatch_layer)
            .init();
        Some(cloudwatch_guard)
    } else {
        tracing_subscriber::registry()
            .with(build_log_filter())
            .with(stdout_layer)
            .init();
        None
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cloudwatch_logging = cloudwatch_logging_config()
        .expect("Failed to parse crawler CloudWatch logging configuration");
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let cloudwatch_client = cloudwatch_logging
        .as_ref()
        .map(|_| CloudWatchLogsClient::new(&aws_config));

    if let (Some(config), Some(client)) = (cloudwatch_logging.as_ref(), cloudwatch_client.as_ref())
    {
        let bootstrap_client = AwsSdkCloudWatchBootstrapClient {
            client: client.clone(),
        };
        ensure_cloudwatch_log_destination(&bootstrap_client, config)
            .await
            .expect("Failed to ensure CloudWatch log group and stream for crawler server");
    }

    let _cloudwatch_guard =
        init_crawler_logging(cloudwatch_logging.as_ref(), cloudwatch_client.clone());

    async move {
        info!("Starting Crawler Server");

        if let Some(config) = cloudwatch_logging.as_ref() {
            info!(
                log_group = %config.log_group_name,
                log_stream = %config.log_stream_name,
                "CloudWatch log export enabled"
            );
        }

        // 1. Build cron config (needed for pool sizing before everything else)
        let config = CrawlerCronConfig {
            spider_interval: Duration::from_hours(72),
            scraper_interval: Duration::from_mins(10),
            scraper_urls_per_domain: 100,
            spider_concurrency: 3,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 3,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            push_batch_size: 1000,
            push_queue_capacity: 2000,
            push_max_batch_age: Duration::from_secs(5),
            push_max_concurrency: 4,
            business_db_max_connections: 8,
            ..Default::default()
        };

        config.validate_business_capacity();

        info!(
            spider_interval_s = config.spider_interval.as_secs(),
            scraper_interval_s = config.scraper_interval.as_secs(),
            spider_concurrency = config.spider_concurrency,
            spider_site_concurrency_limit = config.spider_site_concurrency_limit,
            scraper_concurrency = config.scraper_concurrency,
            scraper_schema_seed_pages = config.scraper_schema_seed_pages,
            scraper_domain_delay_ms = config.scraper_domain_delay.as_millis(),
            scraper_auto_throttle_target_concurrency =
                config.scraper_auto_throttle_target_concurrency,
            scraper_auto_throttle_max_delay_ms = config.scraper_auto_throttle_max_delay.as_millis(),
            scraper_auto_throttle_alpha = config.scraper_auto_throttle_alpha,
            scraper_max_llm_calls_per_listing_source =
                config.scraper_max_llm_calls_per_listing_source,
            push_batch_size = config.effective_push_batch_size(),
            push_queue_capacity = config.effective_push_queue_capacity(),
            push_max_batch_age_ms = config.effective_push_max_batch_age().as_millis(),
            push_max_concurrency = config.effective_push_max_concurrency(),
            business_db_max_connections = config.effective_business_db_max_connections(),
            "Crawler cron configuration loaded"
        );

        // 2. Connect to database — pool is sized to spider_concurrency + scraper_concurrency + 10
        //    to keep headroom for concurrent repository queries.
        let db_url = local_db_url_from_env().unwrap_or_else(|error| {
            panic!("invalid crawler-local database configuration: {error}")
        });
        let pool = config
            .connect_pool(&db_url)
            .await
            .expect("Failed to connect to database");

        info!(
            max_connections = config.effective_db_max_connections(),
            "Connected to crawler-local Postgres"
        );

        // 3. Apply pending crawler-local migrations.
        apply_local_migrations(&pool).await;

        let business_database_url = std::env::var("BUSINESS_DATABASE_URL")
            .expect("BUSINESS_DATABASE_URL environment variable must be set");
        let business_db_max_connections = config.effective_business_db_max_connections();
        let business_pool = PgPoolOptions::new()
            .max_connections(business_db_max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&business_database_url)
            .await
            .expect("Failed to connect to authoritative business Postgres");
        info!(
            max_connections = business_db_max_connections,
            raw_capture_max_concurrency = config.effective_push_max_concurrency(),
            "Connected to authoritative business Postgres"
        );

        let review_config = ReviewServerConfig::from_env().unwrap_or_else(|error| {
            panic!("invalid crawler review console configuration: {error}")
        });
        let runtime_config = CrawlerRuntimeConfig::from_env()
            .unwrap_or_else(|error| panic!("invalid crawler runtime configuration: {error}"));

        // 4. Sync crawler scope from authoritative WebCrawl sources.
        let listing_source_source = Box::new(PostgresWebCrawlSource::new(Box::new(
            SqlxListingSourceReaders::new(business_pool.clone()),
        )));

        // 5. Capture changed crawler evidence through the operational raw-capture use case.
        let raw_capture = Box::new(ProductListingRawCaptureServiceImpl::new(
            Arc::new(CaptureProductListingRawObservationHandler::new(
                SqlxUnitOfWork::new(business_pool.clone()),
                SqlxProductListingRawCaptureWriterFactory::new(),
                SqlxPartnerProductListingAuthorizerFactory::new(),
            )),
            config.effective_push_max_concurrency(),
        ));

        // 6. Wire the shared crawler pipeline and run until stopped.
        crawler::runtime::build(
            pool,
            CrawlerRuntimeProfile {
                cron_config: config,
                review_config,
                state_directory: crawler::runtime::state_directory(),
                listing_source_source,
                raw_capture,
            },
            runtime_config,
        )
        .run()
        .await;
    }
    .instrument(tracing::info_span!("crawler_startup"))
    .await;
}
