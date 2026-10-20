//! Composition root shared by the long-running crawler binaries.
//!
//! `server` and `dev` prepare their crawler-local database and pass a [`CrawlerRuntimeProfile`]
//! carrying only what differs between them. [`CrawlerRuntimeConfig::from_env`] reads the shared
//! settings, [`CrawlerLlmModels::vertex_ai`] creates the provider models, and [`build`] wires
//! everything else once — repositories, the schema service, scraper, spider, URL classification,
//! the LLM governor, the review server and the cron job. [`CrawlerRuntime::run`] then runs the
//! review server and cron loop until either stops.
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
use crate::scraper::scraper_service::{
    ReqwestHtmlFetcher, SchemaLlmReviewMode, ScraperServiceImpl,
};
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
use crate::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiConfigError, CrawlerVertexAiModels};
use large_language_model::{LargeLanguageModel, VertexAiGemini};
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

const SCHEMA_LLM_REVIEW_MODE_ENV: &str = "CRAWLER_SCHEMA_LLM_REVIEW_MODE";

/// Environment settings shared by every crawler entry point.
pub struct CrawlerRuntimeConfig {
    pub review_required: bool,
    pub url_pattern_review_required: bool,
    pub schema_llm_review_mode: SchemaLlmReviewMode,
    pub vertex_ai: CrawlerVertexAiConfig,
    pub vertex_ai_models: CrawlerVertexAiModels,
    pub llm_rate_limit: CrawlerLlmRateLimitConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerRuntimeConfigError {
    #[error(
        "invalid {SCHEMA_LLM_REVIEW_MODE_ENV} value {value:?}; expected one of: {}",
        accepted_schema_llm_review_modes()
    )]
    InvalidSchemaLlmReviewMode { value: String },
    #[error(transparent)]
    VertexAi(#[from] CrawlerVertexAiConfigError),
}

impl CrawlerRuntimeConfig {
    /// Reads the review gates, Vertex AI selection and LLM rate limits, then logs the review
    /// gates and the LLM governor configuration that [`build`] applies.
    pub fn from_env() -> Result<Self, CrawlerRuntimeConfigError> {
        let review_required = env_flag("CRAWLER_REVIEW_REQUIRED");
        let url_pattern_review_required = env_flag("CRAWLER_REVIEW_URL_PATTERN_REQUIRED");
        let (schema_llm_review_mode, schema_llm_review_mode_source) =
            schema_llm_review_mode_from_env()?;
        let vertex_ai = CrawlerVertexAiConfig::from_env()?;
        let vertex_ai_models = CrawlerVertexAiModels::from_env();
        let llm_rate_limit = CrawlerLlmRateLimitConfig::from_env();

        info!(
            review_required,
            url_pattern_review_required,
            schema_llm_review_mode = schema_llm_review_mode.as_str(),
            schema_llm_review_mode_source,
            "Crawler review gates configured"
        );
        info!(
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            url_classification_model = %vertex_ai_models.url_classification,
            max_concurrent_requests = llm_rate_limit.max_concurrent_requests,
            min_request_interval_ms = llm_rate_limit.min_request_interval.as_millis(),
            "Crawler LLM governor configured"
        );

        Ok(Self {
            review_required,
            url_pattern_review_required,
            schema_llm_review_mode,
            vertex_ai,
            vertex_ai_models,
            llm_rate_limit,
        })
    }
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
    pub fn vertex_ai(config: &CrawlerRuntimeConfig) -> Self {
        let models = &config.vertex_ai_models;
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

/// Wires the crawler pipeline on a migrated crawler-local `pool`.
pub fn build<Llm>(
    pool: PgPool,
    profile: CrawlerRuntimeProfile,
    config: CrawlerRuntimeConfig,
    models: CrawlerLlmModels<Llm>,
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
    let CrawlerRuntimeConfig {
        review_required,
        url_pattern_review_required,
        schema_llm_review_mode,
        vertex_ai_models,
        llm_rate_limit,
        ..
    } = config;

    let review_repo = CrawlerReviewRepository::new(pool.clone());
    let llm_governor = Arc::new(CrawlerLlmGovernor::new(llm_rate_limit));

    let normalization_svc = ProductListingNormalizationServiceImpl::new();

    let schema_repo = Box::new(ListingSourceProductSchemaRepositoryImpl::new(Box::leak(
        Box::new(pool.clone()),
    )));
    let schema_svc = ProductListingSchemaServiceImpl::new(
        models.product_schema,
        models.fresh_product_schema,
        schema_repo,
        Some(Arc::clone(&llm_governor)),
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

    let fetcher = Box::new(ReqwestHtmlFetcher::with_auto_throttle_config(
        cron_config.scraper_auto_throttle_config(),
    ));
    let scraper_svc = Box::new(
        ScraperServiceImpl::new_with_schema_seed_pages(
            fetcher,
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
        .with_review_gate(review_repo.clone(), review_required, schema_llm_review_mode),
    );

    let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
    let url_pattern_repo = Box::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone()));

    let class_svc = Box::new(UrlClassificationServiceImpl::new(
        models.url_classification,
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

/// Reads the schema review mode and whether it came from the environment or the default.
/// A set but unrecognized value fails startup instead of silently changing the review policy.
fn schema_llm_review_mode_from_env()
-> Result<(SchemaLlmReviewMode, &'static str), CrawlerRuntimeConfigError> {
    let raw = match std::env::var(SCHEMA_LLM_REVIEW_MODE_ENV) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => {
            return Ok((SchemaLlmReviewMode::default(), "default"));
        }
        Err(std::env::VarError::NotUnicode(raw)) => {
            return Err(CrawlerRuntimeConfigError::InvalidSchemaLlmReviewMode {
                value: raw.to_string_lossy().into_owned(),
            });
        }
    };
    SchemaLlmReviewMode::parse(&raw)
        .map(|mode| (mode, "env"))
        .ok_or(CrawlerRuntimeConfigError::InvalidSchemaLlmReviewMode { value: raw })
}

fn accepted_schema_llm_review_modes() -> String {
    SchemaLlmReviewMode::ALL
        .map(SchemaLlmReviewMode::as_str)
        .join(", ")
}

/// Reads a boolean review-gate switch; unset or unrecognized values keep the gate off.
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::listing_source_registration::{
        ListingSourceSyncError, RegisteredListingSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use large_language_model::{LargeLanguageModelError, StructuredGenerationRequest};
    use rstest::rstest;
    use serial_test::serial;
    use sqlx::AssertSqlSafe;
    use sqlx::postgres::PgPoolOptions;
    use test_api::get_postgres_client;

    /// A model that fails if the pipeline unexpectedly calls it.
    struct UnusedLlm;

    #[async_trait::async_trait]
    impl LargeLanguageModel for UnusedLlm {
        async fn generate<Output>(
            &self,
            _: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            Err(LargeLanguageModelError::Permanent {
                source: application::error::static_error("LLM should not be called in this test"),
            })
        }
    }

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

    const TEST_FLAG: &str = "CRAWLER_RUNTIME_TEST_FLAG";

    #[rstest]
    #[case::true_lowercase(Some("true"), true)]
    #[case::true_uppercase(Some("TRUE"), true)]
    #[case::one(Some("1"), true)]
    #[case::yes(Some("yes"), true)]
    #[case::false_value(Some("false"), false)]
    #[case::garbage(Some("enabled"), false)]
    #[case::unset(None, false)]
    #[serial]
    fn should_read_review_gate_flag(#[case] value: Option<&str>, #[case] expected: bool) {
        set_env(TEST_FLAG, value);

        let enabled = env_flag(TEST_FLAG);
        set_env(TEST_FLAG, None);

        assert_eq!(enabled, expected);
    }

    #[test]
    #[serial]
    fn should_read_runtime_config_from_env() {
        set_env("CRAWLER_REVIEW_REQUIRED", Some("true"));
        set_env("CRAWLER_REVIEW_URL_PATTERN_REQUIRED", None);
        set_env("VERTEX_AI_PROJECT_ID", Some("test-project"));
        set_env("VERTEX_AI_LOCATION", Some("europe-west3"));

        let config = CrawlerRuntimeConfig::from_env();
        for name in [
            "CRAWLER_REVIEW_REQUIRED",
            "VERTEX_AI_PROJECT_ID",
            "VERTEX_AI_LOCATION",
        ] {
            set_env(name, None);
        }

        let config = config.unwrap_or_else(|error| panic!("runtime config from env: {error}"));
        assert!(config.review_required);
        assert!(!config.url_pattern_review_required);
        assert_eq!(
            config.vertex_ai,
            CrawlerVertexAiConfig::new("test-project", "europe-west3")
        );
    }

    #[test]
    #[serial]
    fn should_reject_runtime_config_without_vertex_ai_project() {
        set_env("VERTEX_AI_PROJECT_ID", None);
        set_env("VERTEX_AI_LOCATION", Some("europe-west3"));

        let config = CrawlerRuntimeConfig::from_env();
        set_env("VERTEX_AI_LOCATION", None);

        assert!(matches!(
            config,
            Err(CrawlerRuntimeConfigError::VertexAi(
                CrawlerVertexAiConfigError::MissingEnvironment {
                    name: "VERTEX_AI_PROJECT_ID"
                }
            ))
        ));
    }

    /// Reads the runtime config with a valid Vertex AI environment and the given review settings.
    fn runtime_config_with_review_env(
        review_required: &str,
        schema_llm_review_mode: Option<&str>,
    ) -> Result<CrawlerRuntimeConfig, CrawlerRuntimeConfigError> {
        set_env("CRAWLER_REVIEW_REQUIRED", Some(review_required));
        set_env(SCHEMA_LLM_REVIEW_MODE_ENV, schema_llm_review_mode);
        set_env("VERTEX_AI_PROJECT_ID", Some("test-project"));
        set_env("VERTEX_AI_LOCATION", Some("europe-west3"));

        let config = CrawlerRuntimeConfig::from_env();
        for name in [
            "CRAWLER_REVIEW_REQUIRED",
            SCHEMA_LLM_REVIEW_MODE_ENV,
            "VERTEX_AI_PROJECT_ID",
            "VERTEX_AI_LOCATION",
        ] {
            set_env(name, None);
        }
        config
    }

    #[rstest]
    #[case::human_only("human_only", SchemaLlmReviewMode::HumanOnly)]
    #[case::report_only("report_only", SchemaLlmReviewMode::ReportOnly)]
    #[case::auto_approve(
        "auto_approve_high_confidence",
        SchemaLlmReviewMode::AutoApproveHighConfidence
    )]
    #[serial]
    fn should_read_schema_llm_review_mode_from_env(
        #[case] value: &str,
        #[case] expected: SchemaLlmReviewMode,
    ) {
        let config = runtime_config_with_review_env("true", Some(value))
            .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(config.schema_llm_review_mode, expected);
    }

    #[rstest]
    #[case::review_required("true")]
    #[case::review_not_required("false")]
    #[serial]
    fn should_require_human_schema_review_when_mode_is_unset(#[case] review_required: &str) {
        let config = runtime_config_with_review_env(review_required, None)
            .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(
            config.schema_llm_review_mode,
            SchemaLlmReviewMode::HumanOnly
        );
    }

    #[rstest]
    #[case::unknown("jev_auto")]
    #[case::empty("")]
    #[serial]
    fn should_reject_invalid_schema_llm_review_mode(#[case] value: &str) {
        let error = match runtime_config_with_review_env("true", Some(value)) {
            Ok(_) => panic!("invalid schema review mode {value:?} must fail startup"),
            Err(error) => error,
        };

        assert!(matches!(
            &error,
            CrawlerRuntimeConfigError::InvalidSchemaLlmReviewMode { value: rejected }
                if rejected == value
        ));
        assert_eq!(
            error.to_string(),
            format!(
                "invalid CRAWLER_SCHEMA_LLM_REVIEW_MODE value {value:?}; expected one of: \
                 human_only, report_only, auto_approve_high_confidence"
            )
        );
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
            CrawlerLlmModels::vertex_ai(&runtime_config());
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
            runtime_config(),
            unused_models(),
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

        build(
            pool,
            profile(review_config),
            runtime_config(),
            unused_models(),
        )
        .run()
        .await;
    }

    /// Creates a dedicated database and migrates it like the binaries do; the lib test
    /// container's default database hosts the business schema.
    async fn migrated_crawler_database() -> (PgPool, PgPool) {
        let admin_pool = get_postgres_client().await;
        let database = format!("crawler_runtime_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .execute(&admin_pool)
            .await
            .unwrap_or_else(|error| panic!("create crawler runtime test database: {error}"));
        let pool = PgPoolOptions::new()
            .connect_with(
                admin_pool
                    .connect_options()
                    .as_ref()
                    .clone()
                    .database(&database),
            )
            .await
            .unwrap_or_else(|error| panic!("connect to crawler runtime test database: {error}"));

        apply_local_migrations(&pool).await;
        (admin_pool, pool)
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

    fn runtime_config() -> CrawlerRuntimeConfig {
        CrawlerRuntimeConfig {
            review_required: true,
            url_pattern_review_required: true,
            schema_llm_review_mode: SchemaLlmReviewMode::default(),
            vertex_ai: CrawlerVertexAiConfig::new("test-project", "europe-west3"),
            vertex_ai_models: CrawlerVertexAiModels {
                product_schema: "schema-model".to_owned(),
                url_classification: "classification-model".to_owned(),
            },
            llm_rate_limit: CrawlerLlmRateLimitConfig::default(),
        }
    }

    fn unused_models() -> CrawlerLlmModels<UnusedLlm> {
        CrawlerLlmModels {
            product_schema: UnusedLlm,
            fresh_product_schema: UnusedLlm,
            url_classification: UnusedLlm,
        }
    }
}
