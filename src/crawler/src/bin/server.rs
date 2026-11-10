//! Production server binary for the crawler.
//!
//! Sets up logging with optional CloudWatch export and crawler-local Postgres, then hands the
//! production profile to [`crawler::runtime`]. The runtime continuously spiders ListingSource
//! websites, scrapes product pages, and submits changed raw ProductListing observations to the
//! shared ingestion queue for asynchronous capture and normalization. The crawler scope comes
//! from the admin API; the host holds no business database credentials. SIGTERM and SIGINT stop it
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
//! | `AURA_API_BASE_URL`             | Required base URL of the Aura Historia API; `https`, or `http` for a loopback host only |
//! | `CRAWLER_API_TOKEN`             | Required bearer token that may read admin ListingSources (`GET /v1/admin/listing-sources`); never logged |
//! | `PRODUCT_LISTING_INGESTION_QUEUE_URL` | Required URL of the product-listing ingestion FIFO queue |
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
//! | `CRAWLER_START_REVIEW_AGENT_RUNNER` | `true` starts `review-agent-runner` from next to this binary as its child in `agent` mode, as `dev` does; unset or `false` leaves it to its own service. Never set it next to a runner service: two runners would work the same reviews. Any other value fails startup |
//! | `CRAWLER_CLOUDWATCH_LOG_GROUP`  | Optional CloudWatch Logs group name for crawler server logs    |
//! | `CRAWLER_CLOUDWATCH_LOG_STREAM` | Optional CloudWatch Logs stream name; defaults to host name    |
//!
//! With `CRAWLER_AUTHORING_MODE` `manual` or `agent` no Vertex AI or LLM setting is read
//! ([`CrawlerRuntimeConfig::from_env`]). The review console settings are in
//! `docs/crawler/operations.md#review-service`.
//!
//! # IAM permissions
//!
//! The crawler's AWS credentials need `sqs:SendMessage` on the ingestion FIFO queue and nothing
//! broader.
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
    HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE, SPIDER_LIBRARY_LOG_DIRECTIVE, cloudwatch_logging_config,
    ensure_cloudwatch_log_destination,
};
use crawler::metrics::{EMF_LOG_DIRECTIVE, EmfPassthroughFormat};
use crawler::review::server::ReviewServerConfig;
use crawler::review_agent_runner::{RunnerChild, runner_env, runner_executable_next_to};
use crawler::runtime::{CrawlerRuntimeConfig, CrawlerRuntimeProfile, apply_local_migrations};
use crawler::scraper::scraper_service::DEFAULT_SCHEMA_SEED_PAGES;
use crawler::service::cron::CrawlerCronConfig;
use crawler::service::listing_source_api::{
    ApiListingSourceRegistrationSource, ListingSourceApiConfig,
};
use crawler::service::raw_capture::{
    ProductListingRawCaptureService, ProductListingRawCaptureServiceImpl,
};
use product_listing_ingestion_sqs::SqsProductListingIngestionPublisher;
use product_listing_service::use_cases::SubmitInternalProductListingIngestionHandler;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

const INGESTION_QUEUE_URL_ENV: &str = "PRODUCT_LISTING_INGESTION_QUEUE_URL";

fn build_log_filter() -> EnvFilter {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let raw_level = configured_log_level
        .as_deref()
        .unwrap_or("info")
        .to_string();
    let directives = format!(
        "{raw_level},{SPIDER_LIBRARY_LOG_DIRECTIVE},sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE},{EMF_LOG_DIRECTIVE}"
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
                .with_span_list(false),
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
                            .with_span_list(false)
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

/// Whether `agent` mode starts the review agent runner as this binary's child.
const START_REVIEW_AGENT_RUNNER_ENV: &str = "CRAWLER_START_REVIEW_AGENT_RUNNER";

/// Opt-in: `true` only. Production runs the runner as its own service, so unset means no child.
fn starts_review_agent_runner(value: Option<String>) -> Result<bool, String> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(false),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(value) => Err(format!(
            "invalid {START_REVIEW_AGENT_RUNNER_ENV}={value:?}: true or false"
        )),
    }
}

/// Starts the review agent runner next to this binary as its child, in the working directory
/// where the runtime wrote the agents' token file.
fn start_review_agent_runner_child(console_port: u16) -> Option<RunnerChild> {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            tracing::warn!(error = %error, "Cannot locate the server's executable; running without review agents");
            return None;
        }
    };
    RunnerChild::start(
        &runner_executable_next_to(&executable),
        &crawler::runtime::state_directory(),
        runner_env(std::env::vars_os(), console_port),
    )
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cloudwatch_logging = cloudwatch_logging_config()
        .expect("Failed to parse crawler CloudWatch logging configuration");
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_11_10())
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
            spider_interval: Duration::from_mins(1),
            scraper_interval: Duration::from_mins(1),
            scraper_urls_per_source: 100,
            spider_concurrency: 70,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 70,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            push_batch_size: 500,
            push_queue_capacity: 1000,
            push_max_batch_age: Duration::from_mins(20),
            push_max_concurrency: 1,
            ..Default::default()
        };

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

        let review_config = ReviewServerConfig::from_env().unwrap_or_else(|error| {
            panic!("invalid crawler review console configuration: {error}")
        });
        let runtime_config = CrawlerRuntimeConfig::from_env()
            .unwrap_or_else(|error| panic!("invalid crawler runtime configuration: {error}"));
        let start_review_agent_runner = runtime_config.authoring.has_review_agents()
            && starts_review_agent_runner(std::env::var(START_REVIEW_AGENT_RUNNER_ENV).ok())
                .unwrap_or_else(|error| panic!("invalid crawler configuration: {error}"));
        let console_port = review_config.bind_addr.port();

        // 4. Sync crawler scope from the admin API's WEB_CRAWL ListingSources.
        let api_config = ListingSourceApiConfig::from_env()
            .unwrap_or_else(|error| panic!("invalid crawler API configuration: {error}"));
        let listing_source_source = Box::new(
            ApiListingSourceRegistrationSource::new(api_config)
                .unwrap_or_else(|error| panic!("invalid crawler API configuration: {error}")),
        );

        // 5. Submit changed crawler evidence to the shared ingestion queue.
        let queue_url = std::env::var(INGESTION_QUEUE_URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| panic!("{INGESTION_QUEUE_URL_ENV} must be set"));
        let raw_capture: Box<dyn ProductListingRawCaptureService> =
            Box::new(ProductListingRawCaptureServiceImpl::new(
                Arc::new(SubmitInternalProductListingIngestionHandler::new(
                    SqsProductListingIngestionPublisher::new(
                        aws_sdk_sqs::Client::new(&aws_config),
                        queue_url,
                    ),
                )),
                config.effective_push_max_concurrency(),
            ));

        // 6. Wire the shared crawler pipeline and run until stopped.
        // Building the runtime writes the agents' token file the runner reads.
        let runtime = crawler::runtime::build(
            pool,
            CrawlerRuntimeProfile {
                cron_config: config,
                review_config,
                state_directory: crawler::runtime::state_directory(),
                listing_source_source,
                raw_capture,
            },
            runtime_config,
        );
        let review_agent_runner = if start_review_agent_runner {
            start_review_agent_runner_child(console_port)
        } else {
            None
        };
        runtime.run().await;
        if let Some(runner) = review_agent_runner {
            runner.stop().await;
        }
    }
    .instrument(tracing::info_span!("crawler_startup"))
    .await;
}

#[cfg(test)]
mod tests {
    use super::starts_review_agent_runner;

    #[test]
    fn should_start_the_review_agent_runner_only_when_set_to_true() {
        for (value, expected) in [
            (None, false),
            (Some(""), false),
            (Some("false"), false),
            (Some("TRUE"), true),
        ] {
            assert_eq!(
                Ok(expected),
                starts_review_agent_runner(value.map(str::to_owned))
            );
        }
        assert!(starts_review_agent_runner(Some("yes".to_owned())).is_err());
    }
}
