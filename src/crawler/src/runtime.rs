//! Composition root shared by the long-running crawler binaries.
//!
//! `server` and `dev` prepare their crawler-local database and pass a [`CrawlerRuntimeProfile`]
//! carrying only what differs between them. [`run`] wires everything else once — repositories,
//! the schema service, scraper, spider, URL classification, the LLM governor, the review server
//! and the cron job — and runs the review server and cron loop until either stops.
//!
//! Startup failures panic with a descriptive message, matching the production server's
//! fail-fast startup.

use crate::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crate::local_db::crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl;
use crate::review::repository::CrawlerReviewRepository;
use crate::review::server::{ReviewServer, ReviewServerConfig};
use crate::scraper::candidate_service::ScraperCandidateServiceImpl;
use crate::scraper::css_selector::product_schema_repository::ListingSourceProductSchemaRepositoryImpl;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceImpl;
use crate::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crate::scraper::normalization::product_normalization_service::ProductListingNormalizationServiceImpl;
use crate::scraper::scraper_service::{ReqwestHtmlFetcher, ScraperServiceImpl};
use crate::service::crawler_domain_configuration::CrawlerDomainAdministrationHandler;
use crate::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crate::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource,
};
use crate::service::raw_capture::ProductListingRawCaptureService;
use crate::spider::candidate_service::SpiderCandidateServiceImpl;
use crate::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crate::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crate::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crate::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crate::spider::discovery::website_spider::SpiderImpl;
use crate::spider::local_lock::LocalLockManager;
use crate::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use crate::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

/// What a crawler entry point supplies; everything else is shared wiring.
pub struct CrawlerRuntimeProfile {
    pub cron_config: CrawlerCronConfig,
    pub review_config: ReviewServerConfig,
    /// Source of the ListingSources the crawler is allowed to crawl.
    pub listing_source_source: Box<dyn ListingSourceRegistrationSource>,
    /// Sink for changed raw ProductListing observations.
    pub raw_capture: Box<dyn ProductListingRawCaptureService>,
}

/// Applies pending crawler-local migrations. Runs at startup so deploying a new binary is the
/// only step required to update the crawler schema.
pub async fn apply_local_migrations(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .expect("Failed to run database migrations");
    info!("Crawler-local database migrations applied successfully");
}

/// Wires the crawler pipeline on a migrated crawler-local `pool` and runs the review server and
/// cron loop until either task ends.
pub async fn run(pool: PgPool, profile: CrawlerRuntimeProfile) {
    let CrawlerRuntimeProfile {
        cron_config: config,
        review_config,
        listing_source_source,
        raw_capture,
    } = profile;

    let review_required = crawler_review_required();
    let url_pattern_review_required = crawler_review_url_pattern_required();
    let review_repo = CrawlerReviewRepository::new(pool.clone());

    // Wire scraper + spider dependencies. Provider and model choices stay here; crawler services
    // depend only on the generic LargeLanguageModel capability.
    let vertex_ai_config = CrawlerVertexAiConfig::from_env()
        .expect("VERTEX_AI_PROJECT_ID and VERTEX_AI_LOCATION must be set");
    let vertex_ai_models = CrawlerVertexAiModels::from_env();
    let llm_governor = llm_governor(&vertex_ai_models);

    let normalization_svc = ProductListingNormalizationServiceImpl::new();

    let create_schema_llm = vertex_ai_config
        .create_model(vertex_ai_models.product_schema.clone())
        .expect("failed to initialize Vertex AI model for schema generation");
    let single_schema_llm = vertex_ai_config
        .create_model(vertex_ai_models.product_schema.clone())
        .expect("failed to initialize Vertex AI model for fresh schema generation");

    let schema_repo = Box::new(ListingSourceProductSchemaRepositoryImpl::new(Box::leak(
        Box::new(pool.clone()),
    )));
    let schema_svc = ProductListingSchemaServiceImpl::new(
        create_schema_llm,
        single_schema_llm,
        schema_repo,
        Some(Arc::clone(&llm_governor)),
    );
    let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(Box::leak(
        Box::new(pool.clone()),
    )));

    let scraper_candidates = Box::new(
        ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
            pool.clone(),
            config.scraper_max_llm_calls_per_listing_source,
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
                ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
                    pool.clone(),
                    config.scraper_max_llm_calls_per_listing_source,
                ),
            ),
            config.scraper_schema_seed_pages,
            config.scraper_max_llm_calls_per_listing_source,
        )
        .with_removed_page_schema_repository(removed_page_schema_repo)
        .with_review_gate(review_repo.clone(), review_required),
    );

    let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
    let url_pattern_repo = Box::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone()));

    let classification_llm = vertex_ai_config
        .create_model(vertex_ai_models.url_classification.clone())
        .expect("failed to initialize Vertex AI model for URL classification");
    let class_svc = Box::new(UrlClassificationServiceImpl::new(
        classification_llm,
        Some(Arc::clone(&llm_governor)),
    ));

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

    // Sync crawler scope from the profile's ListingSource source.
    let listing_source_repo = Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone()));
    let listing_source_registration =
        ListingSourceRegistrationService::new(listing_source_source, listing_source_repo);

    let db_max_connections = config.effective_db_max_connections();
    let scraper_max_llm_calls_per_listing_source = config.scraper_max_llm_calls_per_listing_source;
    let push_batch_size = config.effective_push_batch_size();
    let push_queue_capacity = config.effective_push_queue_capacity();
    let push_max_batch_age_ms = config.effective_push_max_batch_age().as_millis();
    let push_max_concurrency = config.effective_push_max_concurrency();
    let business_db_max_connections = config.effective_business_db_max_connections();

    let cron_job = CrawlerCronJob::new(
        config,
        Arc::new(LocalLockManager::new()),
        spider_candidates,
        spider_svc,
        scraper_candidates,
        scraper_svc,
        listing_source_registration,
        raw_capture,
    );

    // Run forever.
    info!(
        db_max_connections,
        scraper_max_llm_calls_per_listing_source,
        push_batch_size,
        push_queue_capacity,
        push_max_batch_age_ms,
        push_max_concurrency,
        business_db_max_connections,
        llm_provider = "vertex_ai",
        schema_model = %vertex_ai_models.product_schema,
        url_classification_model = %vertex_ai_models.url_classification,
        review_required,
        url_pattern_review_required,
        review_bind_addr = %review_config.bind_addr,
        "Crawler Server is fully initialized. Starting background tasks..."
    );
    let review_server = ReviewServer::new(
        review_repo,
        Arc::new(CrawlerDomainAdministrationHandler::new(Arc::new(
            CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()),
        ))),
        review_config,
    );
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

/// Builds the process-wide governor that bounds every crawler LLM call.
fn llm_governor(vertex_ai_models: &CrawlerVertexAiModels) -> Arc<CrawlerLlmGovernor> {
    let llm_rate_limit_config = CrawlerLlmRateLimitConfig::from_env();
    let llm_governor = Arc::new(CrawlerLlmGovernor::new(llm_rate_limit_config));

    info!(
        llm_provider = "vertex_ai",
        schema_model = %vertex_ai_models.product_schema,
        url_classification_model = %vertex_ai_models.url_classification,
        max_concurrent_requests = llm_rate_limit_config.max_concurrent_requests,
        min_request_interval_ms = llm_rate_limit_config.min_request_interval.as_millis(),
        "Crawler LLM governor configured"
    );

    llm_governor
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
