//! Local development binary — runs the full crawler pipeline (spider + scraper) against a set of hardcoded
//! antique ListingSources without needing a running ListingSource service.
//!
//! Unless `LOCAL_DB_URL` names another database, on startup it automatically runs
//! `docker compose up -d` (using the `docker-compose.yml` inside the `crawler` crate), creates
//! `crawler_demo` and waits for Postgres to become ready before applying migrations. No manual
//! setup required — just:
//!
//! ```powershell
//! gcloud auth application-default login
//! $env:VERTEX_AI_PROJECT_ID="my-project"
//! $env:VERTEX_AI_LOCATION="europe-west3"
//! cargo run -p crawler --bin dev
//! ```
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default                                          |
//! |------------------|--------------------------------------|--------------------------------------------------|
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required unless the authoring mode is `manual`)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required unless the authoring mode is `manual`)* |
//! | `SPIDER_MAX_SIZE_BYTES` | Spider page-body ceiling in bytes, from `1048576` (1 MiB) through `8388608` (8 MiB, the crawler's own response-body limit); every crawl fails while it is out of range. `server` requires it; this binary defaults it | `8388608` |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Schema generation/repair model | `gemini-3.5-flash` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOCAL_DB_URL`   | Database to run against instead of `crawler_demo`, e.g. a copy of it. The run then neither starts Docker nor creates the database; a value that is not a connection URL fails startup | `postgres://postgres:postgres@localhost:5432/crawler_demo` |
//! | `CRAWLER_AUTHORING_MODE` | `auto` (LLM, high-confidence schemas auto-approved), `review` (LLM, human approves) or `manual` (no LLM, operator writes patterns and schemas); invalid values fail startup | `review` |
//! | `CRAWLER_REVIEW_REQUIRED`, `CRAWLER_REVIEW_URL_PATTERN_REQUIRED`, `CRAWLER_SCHEMA_LLM_REVIEW_MODE` | Removed: any non-empty value fails startup. Replace them with `CRAWLER_AUTHORING_MODE` (mapping in `docs/crawler/operations.md#authoring-modes`) | unset |
//! | `CRAWLER_REVIEW_BIND_ADDR` | Review UI bind address        | `127.0.0.1:7878`                                |
//! | `CRAWLER_REVIEW_AUTH_TOKEN` | Optional bearer token for the review UI/API; unset allows same-origin local mutations | unset |
//! | `LOG_LEVEL`      | Global log level                     | `info`                                           |
//! | `CRAWLER_LOG_LEVEL` | Crawler-internal log level        | `info`                                           |
//!
//! The pipeline is wired by [`crawler::runtime`], shared with `server`. This binary only supplies
//! the demo ListingSources, short cron intervals, the tokenless local review console, and a file
//! sink: changed raw observations are written to `scraped_raw_observations.json` instead of
//! calling the ProductListing raw-capture use case.
//!
//! # Stopping
//!
//! Ctrl+C (SIGINT) or SIGTERM stops the binary gracefully: no new spider or scraper work starts,
//! running work gets 30 seconds to finish and the raw observations captured so far are written,
//! then the process exits 0. A second Ctrl+C exits at once. The contract is in
//! `docs/crawler/operations.md#stopping-the-crawler`.

use listing_source_core::ListingSourceId;
use sqlx::postgres::PgConnectOptions;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crawler::local_db::{
    DEMO_DB_NAME, LOCAL_DB_URL_ENV, LocalDbUrlError, bootstrap_local_database,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl, demo_db_url,
    local_db_url_from_env,
};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::metrics::{EMF_LOG_DIRECTIVE, EmfPassthroughFormat};
use crawler::review::server::ReviewServerConfig;
use crawler::runtime::{CrawlerRuntimeConfig, CrawlerRuntimeProfile, apply_local_migrations};
use crawler::scraper::scraper_service::DEFAULT_SCHEMA_SEED_PAGES;
use crawler::service::crawler_domain_configuration::CrawlerDomainConfigurationRepository;
use crawler::service::cron::CrawlerCronConfig;
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource, ListingSourceSyncError, RegisteredListingSource,
};
use crawler::service::raw_capture::FileProductListingRawCaptureService;

use tracing::{Instrument, error, info};

// ---------------------------------------------------------------------------
// Demo ListingSource source — returns hardcoded ListingSources (no OpenSearch needed)
// ---------------------------------------------------------------------------

struct DemoListingSourceSource {
    listing_sources: Vec<RegisteredListingSource>,
}

#[async_trait]
impl ListingSourceRegistrationSource for DemoListingSourceSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(self.listing_sources.clone())
    }
}

fn demo_listing_sources() -> Vec<RegisteredListingSource> {
    [
        (1, "Hingstons Antiques", "hingstons-antiques"),
        (
            2,
            "Harrison Antique Furniture",
            "harrison-antique-furniture",
        ),
        (3, "Collinge Antiques", "collinge-antiques"),
    ]
    .into_iter()
    .map(
        |(index, listing_source_name, listing_source_slug)| RegisteredListingSource {
            listing_source_id: ListingSourceId::try_from(
                uuid::Uuid::parse_str(&format!("01890a5d-ac96-774b-af1d-d5586c639f7{index}"))
                    .unwrap_or_else(|error| panic!("invalid demo ListingSource ID: {error}")),
            )
            .unwrap_or_else(|error| panic!("invalid demo ListingSource ID version: {error}")),
            listing_source_name: listing_source_core::ListingSourceName::try_from(
                listing_source_name,
            )
            .unwrap_or_else(|error| panic!("invalid demo ListingSource name: {error}")),
            listing_source_slug: listing_source_core::ListingSourceSlugId::raw(listing_source_slug)
                .unwrap_or_else(|error| panic!("invalid demo ListingSource slug: {error}")),
            crawl_enabled: true,
            fallback_currency: None,
        },
    )
    .collect()
}

/// The spider reads this on every crawl and fails the crawl while it is
/// unset; `server` requires it explicitly, the dev binary defaults it.
const SPIDER_MAX_SIZE_BYTES_ENV: &str = "SPIDER_MAX_SIZE_BYTES";
/// The crawler's own response-body limit: the largest accepted value.
const DEV_SPIDER_MAX_SIZE_BYTES: &str = "8388608";

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();
    default_spider_body_limit();
    let database = DevDatabase::from_env()
        .unwrap_or_else(|error| panic!("invalid crawler-local database configuration: {error}"));
    let span = tracing::info_span!(
        "crawler_dev",
        entrypoint = "dev",
        database = database.name()
    );

    async move {
        // Fail fast on invalid runtime settings before starting Docker or touching the database.
        let runtime_config = CrawlerRuntimeConfig::from_env()
            .unwrap_or_else(|error| panic!("invalid crawler runtime configuration: {error}"));

        let config = CrawlerCronConfig {
            spider_interval: Duration::from_secs(120),
            scraper_interval: Duration::from_secs(30),
            scraper_urls_per_domain: 50,
            spider_concurrency: 100,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 10,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            ..Default::default()
        };

        let db_url = match database.prepare().await {
            Ok(db_url) => db_url,
            Err(error) => {
                error!(error = ?error, "Failed to bootstrap local Postgres database");
                return;
            }
        };

        info!("Waiting for Postgres to be ready…");
        let pool = match connect_with_retry(&config, &db_url).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to connect to Postgres after retries");
                return;
            }
        };

        apply_local_migrations(&pool).await;

        // Seed the demo ListingSources and their crawler domains before the first cron cycle.
        let listing_source_source = Box::new(DemoListingSourceSource {
            listing_sources: demo_listing_sources(),
        });
        let listing_source_repo =
            Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone()));
        let listing_source_registration =
            ListingSourceRegistrationService::new(listing_source_source, listing_source_repo);
        if let Err(error) = listing_source_registration.sync().await {
            error!(error = ?error, "Failed to synchronize demo ListingSources");
            return;
        }
        let domain_configuration =
            Arc::new(CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()));
        for (listing_source, domain) in demo_listing_sources().into_iter().zip([
            "hingstons-antiques.co.uk",
            "harrisonantiquefurniture.co.uk",
            "collingeantiques.com",
        ]) {
            if let Err(error) = domain_configuration
                .register(
                    listing_source.listing_source_id,
                    listing_source_core::Domain::try_from(domain)
                        .unwrap_or_else(|error| panic!("invalid demo crawler domain: {error}")),
                )
                .await
            {
                error!(error = ?error, domain, "Failed to configure demo crawler domain");
                return;
            }
        }

        let review_config = ReviewServerConfig::from_env()
            .unwrap_or_else(|error| panic!("invalid crawler review console configuration: {error}"))
            .allow_local_mutations_without_token();

        info!(
            listing_source_count = demo_listing_sources().len(),
            review_local_mutations_without_token = review_config.local_mutations_without_token(),
            "Crawler dev seeded demo ListingSources. Press Ctrl+C to stop."
        );

        crawler::runtime::build(
            pool,
            CrawlerRuntimeProfile {
                cron_config: config,
                review_config,
                listing_source_source: Box::new(DemoListingSourceSource {
                    listing_sources: demo_listing_sources(),
                }),
                raw_capture: Box::new(FileProductListingRawCaptureService::new(
                    "scraped_raw_observations.json",
                )),
            },
            runtime_config,
        )
        .run()
        .await;
    }
    .instrument(span)
    .await;
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// The crawler-local database a dev run uses. No `Debug` outside tests: the URL holds the
/// database password.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum DevDatabase {
    /// `crawler_demo` in the crawler's Docker Compose Postgres, which the run starts and creates.
    Demo,
    /// The database `LOCAL_DB_URL` names, for example a copy of `crawler_demo`. Whoever sets the
    /// variable provides the database, as for `server`: the run neither starts Docker nor creates it.
    Configured { url: String, name: String },
}

/// Why `LOCAL_DB_URL` cannot select the dev database. Never carries the value: the URL holds the
/// database password.
#[derive(Debug, thiserror::Error)]
enum DevDatabaseError {
    #[error(transparent)]
    Unreadable(#[from] LocalDbUrlError),
    #[error("{LOCAL_DB_URL_ENV} must be a Postgres connection URL")]
    NotConnectionUrl(#[source] sqlx::Error),
}

impl DevDatabase {
    fn from_env() -> Result<Self, DevDatabaseError> {
        Self::from_local_db_url(local_db_url_from_env())
    }

    /// Falls back to `crawler_demo` only while `LOCAL_DB_URL` is missing: an unusable value fails
    /// startup instead of running against the demo state.
    fn from_local_db_url(url: Result<String, LocalDbUrlError>) -> Result<Self, DevDatabaseError> {
        let url = match url {
            Ok(url) => url,
            Err(LocalDbUrlError::Missing) => return Ok(Self::Demo),
            Err(error) => return Err(error.into()),
        };
        let options =
            PgConnectOptions::from_str(&url).map_err(DevDatabaseError::NotConnectionUrl)?;
        // Postgres connects to the database named like the user when the URL names none.
        let name = options
            .get_database()
            .unwrap_or(options.get_username())
            .to_owned();
        Ok(Self::Configured { url, name })
    }

    /// The database name, which logs carry instead of the URL.
    fn name(&self) -> &str {
        match self {
            Self::Demo => DEMO_DB_NAME,
            Self::Configured { name, .. } => name,
        }
    }

    /// Starts and creates the demo database when the run uses it, and returns the URL to connect to.
    async fn prepare(&self) -> Result<String, String> {
        match self {
            Self::Demo => {
                bootstrap_local_database(DEMO_DB_NAME).await?;
                Ok(demo_db_url())
            }
            Self::Configured { url, .. } => Ok(url.clone()),
        }
    }
}

/// Attempts to connect to Postgres, retrying with exponential back-off.
/// This handles the window between `docker compose up -d` returning and
/// Postgres actually accepting connections.
#[tracing::instrument(skip_all)]
async fn connect_with_retry(
    config: &CrawlerCronConfig,
    db_url: &str,
) -> Result<sqlx::PgPool, String> {
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(200);

    loop {
        attempt += 1;
        match config.connect_pool(db_url).await {
            Ok(pool) => {
                info!(
                    attempt,
                    max_connections = config.effective_db_max_connections(),
                    "Connected to Postgres"
                );
                return Ok(pool);
            }
            Err(e) if attempt < 30 => {
                info!(attempt, error = %e, "Postgres not ready yet, retrying…");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
            }
            Err(e) => {
                return Err(format!(
                    "Could not connect to Postgres after {attempt} attempts: {e}"
                ));
            }
        }
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let crawler_level = env::var("CRAWLER_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{raw_level},crawler={crawler_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE},{EMF_LOG_DIRECTIVE}"
    ));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .event_format(EmfPassthroughFormat::new(
            tracing_subscriber::fmt::format().json(),
        ))
        .init();
}

/// Gives an unset `SPIDER_MAX_SIZE_BYTES` the dev default so a local run
/// crawls out of the box; an explicit value, valid or not, is left for the
/// spider to validate.
fn default_spider_body_limit() {
    if env::var_os(SPIDER_MAX_SIZE_BYTES_ENV).is_some() {
        return;
    }
    // SAFETY: runs at startup, like `dotenvy::dotenv()` above, before the
    // runtime executes any task that reads the environment.
    unsafe { env::set_var(SPIDER_MAX_SIZE_BYTES_ENV, DEV_SPIDER_MAX_SIZE_BYTES) };
    info!(
        variable = SPIDER_MAX_SIZE_BYTES_ENV,
        value = DEV_SPIDER_MAX_SIZE_BYTES,
        "Spider page-body limit unset; using the dev default"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        DEV_SPIDER_MAX_SIZE_BYTES, DevDatabase, SPIDER_MAX_SIZE_BYTES_ENV,
        default_spider_body_limit,
    };
    use crawler::local_db::LocalDbUrlError;
    use rstest::rstest;
    use std::env;
    use std::error::Error;

    #[test]
    fn should_use_crawler_demo_when_local_db_url_is_missing() {
        let database = DevDatabase::from_local_db_url(Err(LocalDbUrlError::Missing));

        assert!(matches!(database, Ok(DevDatabase::Demo)));
    }

    #[test]
    fn should_use_the_database_local_db_url_names_when_it_is_set() {
        let url = "postgres://postgres:postgres@localhost:5432/crawler_2541";

        let database = DevDatabase::from_local_db_url(Ok(url.to_owned()))
            .unwrap_or_else(|error| panic!("a Postgres URL must select its database: {error}"));

        assert_eq!(
            database,
            DevDatabase::Configured {
                url: url.to_owned(),
                name: "crawler_2541".to_owned(),
            }
        );
    }

    #[rstest]
    #[case::not_unicode(Err(LocalDbUrlError::NotUnicode))]
    #[case::not_a_url(Ok("postgres://postgres:secret@localhost:port/crawler_2541".to_owned()))]
    fn should_fail_startup_without_echoing_the_url_when_local_db_url_is_unusable(
        #[case] url: Result<String, LocalDbUrlError>,
    ) {
        let error = DevDatabase::from_local_db_url(url)
            .err()
            .unwrap_or_else(|| {
                panic!("an unusable LOCAL_DB_URL must not fall back to crawler_demo")
            });

        let mut cause: Option<&dyn Error> = Some(&error);
        while let Some(current) = cause {
            assert!(!current.to_string().contains("secret"), "{current}");
            cause = current.source();
        }
    }

    /// One test owns the variable: parallel tests would race on process state.
    #[test]
    fn should_default_the_spider_body_limit_only_while_unset() {
        // SAFETY: the only test in this binary that touches the environment.
        unsafe { env::remove_var(SPIDER_MAX_SIZE_BYTES_ENV) };
        default_spider_body_limit();
        assert_eq!(
            env::var(SPIDER_MAX_SIZE_BYTES_ENV).as_deref(),
            Ok(DEV_SPIDER_MAX_SIZE_BYTES)
        );

        unsafe { env::set_var(SPIDER_MAX_SIZE_BYTES_ENV, "1048576") };
        default_spider_body_limit();
        assert_eq!(
            env::var(SPIDER_MAX_SIZE_BYTES_ENV).as_deref(),
            Ok("1048576")
        );
        unsafe { env::remove_var(SPIDER_MAX_SIZE_BYTES_ENV) };
    }
}
