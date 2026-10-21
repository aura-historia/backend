//! Composition root shared by the long-running crawler binaries.
//!
//! `server` and `dev` prepare their crawler-local database and pass a [`CrawlerRuntimeProfile`]
//! carrying only what differs between them. [`CrawlerRuntimeConfig::from_env`] reads the shared
//! settings and [`build`] wires everything else once — the LLM models selected by
//! [`CrawlerLlmConfig`], repositories, the schema service, scraper, spider, URL classification,
//! the LLM governor, the review server and the cron job. [`CrawlerRuntime::run`] then runs the
//! review server and cron loop until either stops.
//!
//! Startup failures panic with a descriptive message, matching the production server's
//! fail-fast startup.

use crate::authoring::CrawlerAuthoringMode;
use crate::llm_runtime::{CrawlerLlmGovernor, DisabledLlm};
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
use crate::spider::classification::url_pattern_service::{
    UrlPatternAuthoring, UrlPatternServiceImpl,
};
use crate::spider::discovery::website_spider::SpiderImpl;
use crate::spider::local_lock::LocalLockManager;
use crate::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use large_language_model::{LargeLanguageModel, VertexAiGemini};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

mod config;

pub use config::{
    ConfigSource, CrawlerLlmConfig, CrawlerRuntimeConfig, CrawlerRuntimeConfigError,
    CrawlerVertexAiLlmConfig,
};

/// What a crawler entry point supplies; everything else is shared wiring.
pub struct CrawlerRuntimeProfile {
    pub cron_config: CrawlerCronConfig,
    pub review_config: ReviewServerConfig,
    /// Source of the ListingSources the crawler is allowed to crawl.
    pub listing_source_source: Box<dyn ListingSourceRegistrationSource>,
    /// Sink for changed raw ProductListing observations.
    pub raw_capture: Box<dyn ProductListingRawCaptureService>,
}

/// The language models the crawler pipeline uses.
pub struct CrawlerLlmModels<Llm> {
    pub product_schema: Llm,
    pub fresh_product_schema: Llm,
    pub url_classification: Llm,
}

impl CrawlerLlmModels<VertexAiGemini> {
    /// Creates the Vertex AI models selected by `config`. Provider and model choices stay here;
    /// crawler services depend only on the generic LargeLanguageModel capability.
    pub fn vertex_ai(config: &CrawlerVertexAiLlmConfig) -> Self {
        let models = &config.models;
        Self {
            product_schema: config
                .vertex_ai
                .create_model(models.product_schema.clone())
                .expect("failed to initialize Vertex AI model for schema generation"),
            fresh_product_schema: config
                .vertex_ai
                .create_model(models.product_schema.clone())
                .expect("failed to initialize Vertex AI model for fresh schema generation"),
            url_classification: config
                .vertex_ai
                .create_model(models.url_classification.clone())
                .expect("failed to initialize Vertex AI model for URL classification"),
        }
    }
}

impl CrawlerLlmModels<DisabledLlm> {
    fn disabled() -> Self {
        Self {
            product_schema: DisabledLlm,
            fresh_product_schema: DisabledLlm,
            url_classification: DisabledLlm,
        }
    }
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

/// The wired review server and cron job, ready to run.
pub struct CrawlerRuntime {
    review_server: ReviewServer,
    cron_job: CrawlerCronJob,
}

/// Wires the crawler pipeline on a migrated crawler-local `pool`, creating the LLM models that
/// `config` selects.
pub fn build(
    pool: PgPool,
    profile: CrawlerRuntimeProfile,
    config: CrawlerRuntimeConfig,
) -> CrawlerRuntime {
    let CrawlerRuntimeConfig { authoring, llm } = config;
    match llm {
        CrawlerLlmConfig::VertexAi(vertex) => {
            let governor = Arc::new(CrawlerLlmGovernor::new(vertex.rate_limit));
            let models = CrawlerLlmModels::vertex_ai(&vertex);
            wire(pool, profile, authoring, models, Some(governor))
        }
        CrawlerLlmConfig::Disabled => {
            wire(pool, profile, authoring, CrawlerLlmModels::disabled(), None)
        }
    }
}

fn wire<Llm>(
    pool: PgPool,
    profile: CrawlerRuntimeProfile,
    authoring: CrawlerAuthoringMode,
    models: CrawlerLlmModels<Llm>,
    llm_governor: Option<Arc<CrawlerLlmGovernor>>,
) -> CrawlerRuntime
where
    Llm: LargeLanguageModel + 'static,
{
    let CrawlerRuntimeProfile {
        cron_config,
        review_config,
        listing_source_source,
        raw_capture,
    } = profile;
    let review_repo = CrawlerReviewRepository::new(pool.clone());

    let normalization_svc = ProductListingNormalizationServiceImpl::new();

    let schema_repo = Box::new(ListingSourceProductSchemaRepositoryImpl::new(Box::leak(
        Box::new(pool.clone()),
    )));
    let schema_svc = ProductListingSchemaServiceImpl::new(
        models.product_schema,
        models.fresh_product_schema,
        schema_repo,
        llm_governor.clone(),
    );
    let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(Box::leak(
        Box::new(pool.clone()),
    )));

    let scraper_candidates = Box::new(
        ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
            pool.clone(),
            cron_config.scraper_max_llm_calls_per_listing_source,
        ),
    );

    // One fetcher for scraper and review server: both requests count against
    // the same robots.txt cache and per-domain pacing.
    let fetcher = Arc::new(ReqwestHtmlFetcher::with_auto_throttle_config(
        cron_config.scraper_auto_throttle_config(),
    ));
    let scraper_svc = Box::new(
        ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(Arc::clone(&fetcher)),
            Box::new(schema_svc),
            Box::new(normalization_svc),
            Arc::new(
                ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
                    pool.clone(),
                    cron_config.scraper_max_llm_calls_per_listing_source,
                ),
            ),
            cron_config.scraper_schema_seed_pages,
            cron_config.scraper_max_llm_calls_per_listing_source,
        )
        .with_removed_page_schema_repository(removed_page_schema_repo)
        .with_review_gate(review_repo.clone(), authoring),
    );

    let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
    let url_pattern_repo = Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone()));

    let url_pattern_authoring = match authoring {
        CrawlerAuthoringMode::Manual => UrlPatternAuthoring::Manual {
            review_repository: review_repo.clone(),
        },
        CrawlerAuthoringMode::Auto | CrawlerAuthoringMode::Review => {
            UrlPatternAuthoring::Inferred {
                classification_service: Box::new(UrlClassificationServiceImpl::new(
                    models.url_classification,
                    llm_governor,
                )),
                review_repository: Some(review_repo.clone()),
                apply_directly: authoring == CrawlerAuthoringMode::Auto,
            }
        }
    };
    let pattern_svc = Box::new(UrlPatternServiceImpl::new(
        url_pattern_repo,
        url_pattern_authoring,
    ));

    let spider_config = SpiderServiceConfig {
        ..Default::default()
    };
    let website_spider = Box::new(SpiderImpl::new(cron_config.spider_website_config()));

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

    let db_max_connections = cron_config.effective_db_max_connections();
    let scraper_max_llm_calls_per_listing_source =
        cron_config.scraper_max_llm_calls_per_listing_source;
    let push_batch_size = cron_config.effective_push_batch_size();
    let push_queue_capacity = cron_config.effective_push_queue_capacity();
    let push_max_batch_age_ms = cron_config.effective_push_max_batch_age().as_millis();
    let push_max_concurrency = cron_config.effective_push_max_concurrency();
    let business_db_max_connections = cron_config.effective_business_db_max_connections();

    let cron_job = CrawlerCronJob::new(
        cron_config,
        Arc::new(LocalLockManager::new()),
        spider_candidates,
        spider_svc,
        scraper_candidates,
        scraper_svc,
        listing_source_registration,
        raw_capture,
    );

    info!(
        db_max_connections,
        scraper_max_llm_calls_per_listing_source,
        push_batch_size,
        push_queue_capacity,
        push_max_batch_age_ms,
        push_max_concurrency,
        business_db_max_connections,
        authoring_mode = authoring.as_str(),
        review_bind_addr = %review_config.bind_addr,
        "Crawler Server is fully initialized. Starting background tasks..."
    );
    let review_server = ReviewServer::new(
        review_repo,
        Arc::new(CrawlerDomainAdministrationHandler::new(Arc::new(
            CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()),
        ))),
        review_config,
        fetcher,
    );

    CrawlerRuntime {
        review_server,
        cron_job,
    }
}

impl CrawlerRuntime {
    /// Runs the review server and cron loop until either task ends.
    pub async fn run(self) {
        let Self {
            review_server,
            cron_job,
        } = self;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_runtime::CrawlerLlmRateLimitConfig;
    use crate::service::listing_source_registration::{
        ListingSourceSyncError, RegisteredListingSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use crate::test_support::migrated_crawler_database;
    use crate::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};
    use serial_test::serial;

    /// A crawl scope without ListingSources, so the cron loop finds no work.
    struct EmptyListingSources;

    #[async_trait::async_trait]
    impl ListingSourceRegistrationSource for EmptyListingSources {
        async fn fetch_registered_listing_sources(
            &self,
        ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
            Ok(Vec::new())
        }
    }

    // Tests that touch the process environment or the shared Postgres container are serialized,
    // so no test reads the environment while another one changes it.
    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    #[serial]
    fn should_fail_fast_when_vertex_ai_credentials_are_unavailable() {
        let configured_credentials = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
        let missing_credentials = std::env::temp_dir().join(format!(
            "crawler-runtime-missing-credentials-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        set_env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            missing_credentials.to_str(),
        );

        let result = std::panic::catch_unwind(|| {
            CrawlerLlmModels::vertex_ai(&vertex_config());
        });
        set_env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            configured_credentials.as_deref(),
        );

        let panic = result.expect_err("model creation must fail without credentials");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or_default();
        assert!(
            message.starts_with("failed to initialize Vertex AI model for schema generation"),
            "unexpected panic: {message}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn should_build_runtime_on_freshly_migrated_crawler_database() {
        let (admin_pool, pool) = migrated_crawler_database().await;
        let applied_migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count applied crawler migrations: {error}"));

        let runtime = build(
            pool.clone(),
            profile(review_config()),
            disabled_llm_config(),
        );
        drop(runtime);
        pool.close().await;
        admin_pool.close().await;

        assert!(applied_migrations > 0);
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "crawler review server task panicked")]
    async fn should_stop_when_the_review_server_cannot_start() {
        let (_admin_pool, pool) = migrated_crawler_database().await;
        let occupied = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("reserve review console port: {error}"));
        let mut review_config = review_config();
        review_config.bind_addr = occupied
            .local_addr()
            .unwrap_or_else(|error| panic!("read reserved review console port: {error}"));

        build(pool, profile(review_config), disabled_llm_config())
            .run()
            .await;
    }

    fn review_config() -> ReviewServerConfig {
        set_env("CRAWLER_REVIEW_BIND_ADDR", None);
        set_env("CRAWLER_REVIEW_AUTH_TOKEN", None);
        ReviewServerConfig::from_env()
            .unwrap_or_else(|error| panic!("default review config: {error}"))
    }

    fn profile(review_config: ReviewServerConfig) -> CrawlerRuntimeProfile {
        CrawlerRuntimeProfile {
            cron_config: CrawlerCronConfig::default(),
            review_config,
            listing_source_source: Box::new(EmptyListingSources),
            raw_capture: Box::new(MockProductListingRawCaptureService::new()),
        }
    }

    /// Wires the whole pipeline without a provider: the cron loop finds no work and no test
    /// needs Vertex AI credentials.
    fn disabled_llm_config() -> CrawlerRuntimeConfig {
        CrawlerRuntimeConfig {
            authoring: CrawlerAuthoringMode::Manual,
            llm: CrawlerLlmConfig::Disabled,
        }
    }

    fn vertex_config() -> CrawlerVertexAiLlmConfig {
        CrawlerVertexAiLlmConfig {
            vertex_ai: CrawlerVertexAiConfig::new("test-project", "europe-west3"),
            models: CrawlerVertexAiModels {
                product_schema: "schema-model".to_owned(),
                url_classification: "classification-model".to_owned(),
            },
            rate_limit: CrawlerLlmRateLimitConfig::default(),
        }
    }
}
