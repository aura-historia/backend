//! Production server binary for the crawler.
//!
//! Wires all dependencies (Postgres, OpenSearch, DynamoDB, LLM) and starts the
//! [`CrawlerCronJob`] loop that continuously spiders shop websites, scrapes product pages,
//! and pushes normalized products to DynamoDB via [`CommandProductServiceImpl`].
//!
//! # Connection pool sizing
//!
//! `db_max_connections` defaults to
//! `spider_concurrency + scraper_concurrency + 10`.
//! Override it explicitly in [`CrawlerCronConfig`] if needed.
//!
//! # Required environment variables
//!
//! | Variable                        | Purpose                                                        |
//! |---------------------------------|----------------------------------------------------------------|
//! | `LOCAL_DB_URL`                  | Hardcoded local Postgres URL (`crawler_server`)                |
//! | `GEMINI_API_KEY`                | API key for the Gemini LLM backend                             |
//! | `GEMINI_MODEL`                  | Gemini model name (default: `gemini-3.1-flash-lite-preview`)   |
//! | `GEMINI_CHEAP_MODEL`            | Default cheaper model for low-risk crawler LLM tasks           |
//! | `GEMINI_STATE_MAPPING_MODEL`    | Optional override for product state mapping LLM calls          |
//! | `GEMINI_URL_CLASSIFICATION_MODEL` | Optional override for URL classification LLM calls            |
//! | `GEMINI_FLEX`                   | Enable Gemini Flex inference when set to `true`                |
//! | `GEMINI_MAX_CONCURRENT_REQUESTS`| Max in-flight crawler Gemini calls (default: `1`)              |
//! | `GEMINI_MIN_REQUEST_INTERVAL_MS`| Minimum delay between crawler Gemini request starts (default: `2000`) |
//! | `DYNAMODB_TABLE_NAME`           | DynamoDB table for product events                              |
//! | `OPENSEARCH_ENDPOINT_URL`       | OpenSearch base URL                                            |
//! | `OPENSEARCH_USERNAME`           | OpenSearch username                                            |
//! | `OPENSEARCH_PASSWORD`           | OpenSearch password                                            |
//! | `CRAWLER_CLOUDWATCH_LOG_GROUP`  | Optional CloudWatch Logs group name for crawler server logs    |
//! | `CRAWLER_CLOUDWATCH_LOG_STREAM` | Optional CloudWatch Logs stream name; defaults to host name    |
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
use common::logging::GeminiServiceTier;
use common::pagination::cursor::Cursor;
use common::shop_id::ShopId;
use crawler::google_llm::{
    GeminiRateLimitConfig, GeminiRateLimiter, gemini_flex_enabled, google_llm_builder,
    state_mapping_gemini_model, url_classification_gemini_model,
};
use crawler::local_db::{SERVER_DB_NAME, bootstrap_local_database, server_db_url};
use crawler::logging::{
    CloudWatchBootstrapClient, CloudWatchBootstrapError, CloudWatchLoggingConfig,
    HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE, cloudwatch_logging_config,
    ensure_cloudwatch_log_destination,
};
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::{ReviewServer, ReviewServerConfig};
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_SCHEMA_SEED_PAGES, ReqwestHtmlFetcher, ScraperServiceImpl,
};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::product_push::ProductPushServiceImpl;
use crawler::service::shop_registration::{
    RegisteredShop, ShopRegistrationRepositoryImpl, ShopRegistrationService,
    ShopRegistrationSource, ShopSyncError,
};
use crawler::spider::advisory_lock::LocalLockManager;
use crawler::spider::candidate_service::SpiderCandidateServiceImpl;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ShopUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use fxrate::dynamodb::repository::FxRateDynamoDbRepositoryImpl;
use fxrate::service::FxRateServiceImpl;
use opensearch::auth::Credentials;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use shop::core::partner_status::ShopPartnerStatus;
use shop::core::shop::Shop;
use shop::core::shop_search::ShopSearch;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::query_service::{QueryShopService, QueryShopServiceImpl};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ---------------------------------------------------------------------------
// ShopRegistrationSource backed by QueryShopService (OpenSearch)
// ---------------------------------------------------------------------------

struct OpenSearchShopSource {
    opensearch_client: opensearch::OpenSearch,
}

fn should_sync_shop(shop: &Shop) -> bool {
    !matches!(shop.partner_status, ShopPartnerStatus::Partnered)
        && shop.domains.iter().any(|domain| {
            ["anticoantico.com", "antik-und-stil.com", "antixx.de"].contains(&domain.as_str())
        })
}

#[async_trait]
impl ShopRegistrationSource for OpenSearchShopSource {
    async fn fetch_registered_shops(&self) -> Result<Vec<RegisteredShop>, ShopSyncError> {
        let repository = ShopOpenSearchRepositoryImpl::new(&self.opensearch_client);
        let query_service = QueryShopServiceImpl::new(&repository);

        let search = ShopSearch::default();
        let mut registered_shops = Vec::new();
        let mut cursor: Option<Cursor<serde_json::Value>> = None;

        loop {
            let result = query_service
                .search_shops(&search, &None, &cursor)
                .await
                .map_err(|e| ShopSyncError::FetchError(e.to_string()))?;

            let page_size = result.items.len();
            for shop in result.items {
                if !should_sync_shop(&shop) {
                    continue;
                }

                let slug: String = shop.shop_slug_id.into();
                let name: String = shop.name.into();
                let shop_id: ShopId = shop.shop_id;
                let shop_type = shop.shop_type;

                registered_shops.push(RegisteredShop {
                    shop_id,
                    shop_name: name,
                    shop_slug: slug,
                    shop_type,
                    domains: shop.domains,
                });
            }

            if page_size == 0 || result.cursor.search_after.is_none() {
                break;
            }

            cursor = Some(result.cursor);
        }

        Ok(registered_shops)
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
        "{raw_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    );
    EnvFilter::new(directives)
}

fn init_crawler_logging(
    cloudwatch_config: Option<&CloudWatchLoggingConfig>,
    cloudwatch_client: Option<CloudWatchLogsClient>,
) -> Option<tracing_cloudwatch::CloudWatchWorkerGuard> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_ansi(false);

    if let (Some(config), Some(client)) = (cloudwatch_config, cloudwatch_client) {
        let (cloudwatch_layer, cloudwatch_guard) = tracing_cloudwatch::layer()
            .with_fmt_layer(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(false)
                    .with_ansi(false),
            )
            .with_code_location(false)
            .with_target(false)
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

fn build_opensearch_client() -> opensearch::OpenSearch {
    let endpoint_url_str = std::env::var("OPENSEARCH_ENDPOINT_URL")
        .expect("OPENSEARCH_ENDPOINT_URL environment variable must be set");
    let endpoint_url =
        url::Url::parse(&endpoint_url_str).expect("OPENSEARCH_ENDPOINT_URL must be a valid URL");

    let username = std::env::var("OPENSEARCH_USERNAME").expect("OPENSEARCH_USERNAME must be set");
    let password = std::env::var("OPENSEARCH_PASSWORD").expect("OPENSEARCH_PASSWORD must be set");

    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
        .auth(Credentials::Basic(username, password))
        .build()
        .expect("Failed to build OpenSearch transport");

    opensearch::OpenSearch::new(transport)
}

fn crawler_review_required() -> bool {
    std::env::var("CRAWLER_REVIEW_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn crawler_review_url_pattern_required() -> bool {
    std::env::var("CRAWLER_REVIEW_URL_PATTERN_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
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
            scraper_max_llm_calls_per_shop = config.scraper_max_llm_calls_per_shop,
            "Crawler cron configuration loaded"
        );

        // 2. Connect to database — pool is sized to spider_concurrency + scraper_concurrency + 10
        //    to keep headroom for concurrent repository queries.
        bootstrap_local_database(SERVER_DB_NAME)
            .await
            .expect("Failed to bootstrap local Postgres database");
        let db_url = server_db_url();
        let pool = config
            .connect_pool(&db_url)
            .await
            .expect("Failed to connect to database");

        info!(
            max_connections = config.effective_db_max_connections(),
            "Connected to Postgres"
        );

        // 3. Apply pending migrations — runs at startup so deploying a new binary
        //    is the only step required to update the production schema.
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run database migrations");
        info!("Database migrations applied successfully");

        let review_required = crawler_review_required();
        let url_pattern_review_required = crawler_review_url_pattern_required();
        let review_config =
            ReviewServerConfig::from_env().expect("CRAWLER_REVIEW_BIND_ADDR must be host:port");
        let review_repo = CrawlerReviewRepository::new(pool.clone());

        // 4. Wire scraper + spider dependencies
        let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
        let model = std::env::var("GEMINI_MODEL")
            .unwrap_or_else(|_| "gemini-3.1-pro-preview".to_string());
        let state_model = state_mapping_gemini_model();
        let classification_model = url_classification_gemini_model();
        unsafe {
            std::env::set_var("GEMINI_MODEL", &model);
        }
        let gemini_flex = gemini_flex_enabled();
        let gemini_service_tier = if gemini_flex { "flex" } else { "default" };
        let llm_service_tier = Some(if gemini_flex {
            GeminiServiceTier::Flex
        } else {
            GeminiServiceTier::Standard
        });
        let gemini_rate_limit_config = GeminiRateLimitConfig::from_env();
        let gemini_rate_limiter = Arc::new(GeminiRateLimiter::new(gemini_rate_limit_config));

        info!(
            gemini_model = %model,
            gemini_state_mapping_model = %state_model,
            gemini_url_classification_model = %classification_model,
            gemini_service_tier,
            gemini_max_concurrent_requests = gemini_rate_limit_config.max_concurrent_requests,
            gemini_min_request_interval_ms = gemini_rate_limit_config.min_request_interval.as_millis(),
            "Gemini crawler rate limiter configured"
        );

        let state_llm_builder = google_llm_builder(&api_key, &state_model, gemini_flex);

        let state_mapping_repo = Box::new(ProductStateMappingRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));
        let state_mapping_svc = ProductStateMappingServiceImpl::new(
            state_llm_builder,
            llm_service_tier,
            state_mapping_repo,
            Some(Arc::clone(&gemini_rate_limiter)),
        )
        .expect("failed to build ProductStateMappingServiceImpl");

        let normalization_svc = ProductNormalizationServiceImpl::new(Box::new(state_mapping_svc));

        let create_schema_llm_builder = google_llm_builder(&api_key, &model, gemini_flex);
        let single_schema_llm_builder = google_llm_builder(&api_key, &model, gemini_flex);

        let schema_repo = Box::new(ShopsProductSchemaRepositoryImpl::new(Box::leak(Box::new(
            pool.clone(),
        ))));
        let schema_svc = ProductSchemaServiceImpl::new(
            create_schema_llm_builder,
            single_schema_llm_builder,
            llm_service_tier,
            schema_repo,
            Some(Arc::clone(&gemini_rate_limiter)),
        )
            .expect("failed to build ProductSchemaServiceImpl");
        let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));

        let scraper_candidates = Box::new(
            ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
                pool.clone(),
                config.scraper_max_llm_calls_per_shop,
            ),
        );

        let fetcher = Box::new(ReqwestHtmlFetcher::with_auto_throttle_config(
            config.scraper_auto_throttle_config(),
        ));
        let scraper_svc = Box::new(
            ScraperServiceImpl::new_with_schema_seed_pages(
                fetcher,
                Box::new(schema_svc),
                Box::new(normalization_svc),
                Arc::new(
                    ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
                        pool.clone(),
                        config.scraper_max_llm_calls_per_shop,
                    ),
                ),
                config.scraper_schema_seed_pages,
                config.scraper_max_llm_calls_per_shop,
            )
            .with_removed_page_schema_repository(removed_page_schema_repo)
            .with_review_gate(review_repo.clone(), review_required),
        );

        let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
        let url_pattern_repo = Box::new(ShopUrlPatternRepositoryImpl::new(pool.clone()));

        let class_llm_builder = google_llm_builder(&api_key, &classification_model, gemini_flex);
        let class_svc = Box::new(
            UrlClassificationServiceImpl::new(
                class_llm_builder,
                llm_service_tier,
                Some(Arc::clone(&gemini_rate_limiter)),
            )
            .unwrap(),
        );

        let pattern_svc = Box::new(UrlPatternServiceImpl::new_with_review(
            Arc::new(*url_pattern_repo),
            class_svc,
            review_repo.clone(),
            url_pattern_review_required,
        ));

        let spider_config = SpiderServiceConfig {
            ..Default::default()
        };
        let website_spider = Box::new(SpiderImpl::new(config.spider_website_config()));

        let spider_svc = Box::new(SpiderServiceImpl::new(
            spider_config,
            website_spider,
            pattern_svc,
            url_metadata_repo.clone(),
        ));

        let spider_candidates = Box::new(SpiderCandidateServiceImpl::new(pool.clone()));

        // 5. Wire shop registration (sync from OpenSearch)
        let opensearch_client = build_opensearch_client();
        let shop_source = Box::new(OpenSearchShopSource { opensearch_client });
        let shop_repo = Box::new(ShopRegistrationRepositoryImpl::new(pool.clone()));
        let shop_registration = ShopRegistrationService::new(shop_source, shop_repo);

        // 6. Wire product push — backed by DynamoDB in production
        let table_name =
            std::env::var("DYNAMODB_TABLE_NAME").expect("DYNAMODB_TABLE_NAME must be set");
        let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

        let product_dynamodb_repo = Box::leak(Box::new(ProductDynamoDbRepositoryImpl::new(
            Box::leak(Box::new(dynamodb.clone())),
            table_name.clone(),
        )));
        let shop_dynamodb_repo = Box::leak(Box::new(ShopDynamoDbRepositoryImpl::new(
            Box::leak(Box::new(dynamodb.clone())),
            table_name.clone(),
        )));
        let get_shop_service = Box::leak(Box::new(GetShopServiceImpl::new(shop_dynamodb_repo)));
            let fxrate_repository = Box::leak(Box::new(FxRateDynamoDbRepositoryImpl::new(
            Box::leak(Box::new(dynamodb.clone())),
            table_name.clone(),
        )));
        let fxrate_service = FxRateServiceImpl::new_read_only(fxrate_repository);
        let command_product_service = Box::new(
            CommandProductServiceImpl::new(
                product_dynamodb_repo,
                &fxrate_service,
                get_shop_service,
                    )
            .await
            .expect("shouldn't fail creating CommandProductServiceImpl (check FxRates record in DynamoDB)"),
        );
        let product_push = Box::new(ProductPushServiceImpl::new(command_product_service));

        let db_max_connections = config.effective_db_max_connections();
        let scraper_max_llm_calls_per_shop = config.scraper_max_llm_calls_per_shop;

        // 7. Build cron job
        let cron_job = CrawlerCronJob::new(
            config,
            Arc::new(LocalLockManager::new()),
            spider_candidates,
            spider_svc,
            scraper_candidates,
            scraper_svc,
            shop_registration,
            product_push,
        );

        // 8. Run forever
        info!(
            db_max_connections,
            scraper_max_llm_calls_per_shop,
            gemini_model = %model,
            gemini_state_mapping_model = %state_model,
            gemini_url_classification_model = %classification_model,
            gemini_service_tier,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Crawler Server is fully initialized. Starting background tasks..."
        );
        let review_server = ReviewServer::new(review_repo, review_config);
        let review_handle = tokio::spawn(async move {
            review_server
                .run()
                .await
                .expect("crawler review server failed")
        });
        let cron_handle = tokio::spawn(async move {
            cron_job.run_loop().await;
        });

        tokio::select! {
            result = review_handle => {
                result.expect("crawler review server task panicked");
            }
            result = cron_handle => {
                result.expect("crawler cron task panicked");
            }
        }
    }
    .instrument(tracing::info_span!("crawler_startup"))
    .await;
}

#[cfg(test)]
mod tests {
    use super::should_sync_shop;
    use common::shop_slug_id::ShopSlugId;
    use common::{actor::domain::Actor, domain::Domain, shop_id::ShopId, shop_name::ShopName};
    use shop::core::shop_type::ShopType;
    use shop::core::{partner_status::ShopPartnerStatus, shop::Shop};
    use time::OffsetDateTime;

    fn mk_shop(partner_status: ShopPartnerStatus, domain: &str) -> Shop {
        Shop {
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("test-shop"),
            name: ShopName::from("Test Shop"),
            shop_type: ShopType::CommercialDealer,
            domains: [Domain::try_from(domain).unwrap()].into(),
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: None,
            view_url: None,
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status,
            affiliate_configuration: None,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn should_exclude_partnered_shop_even_when_domain_is_allowed() {
        let shop = mk_shop(ShopPartnerStatus::Partnered, "anticoantico.com");

        assert!(!should_sync_shop(&shop));
    }

    #[test]
    fn should_include_scraped_shop_when_domain_is_allowed() {
        let shop = mk_shop(ShopPartnerStatus::Scraped, "anticoantico.com");

        assert!(should_sync_shop(&shop));
    }

    #[test]
    fn should_exclude_scraped_shop_when_domain_is_not_allowed() {
        let shop = mk_shop(ShopPartnerStatus::Scraped, "example.com");

        assert!(!should_sync_shop(&shop));
    }
}
