//! Local development binary — runs the full crawler pipeline (spider + scraper) against a set of hardcoded
//! antique ListingSources without needing a running ListingSource service.
//!
//! On startup it automatically runs `docker compose up -d` (using the
//! `docker-compose.yml` inside the `crawler` crate) and waits for Postgres to
//! become ready before applying migrations. No manual setup required — just:
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
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required unless the authoring mode is `manual` or `agent`)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required unless the authoring mode is `manual` or `agent`)* |
//! | `SPIDER_MAX_SIZE_BYTES` | Spider page-body ceiling in bytes, from `1048576` (1 MiB) through `8388608` (8 MiB, the crawler's own response-body limit); every crawl fails while it is out of range. `server` requires it; this binary defaults it | `8388608` |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Schema generation/repair model | `gemini-3.5-flash` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL                | `postgres://postgres:postgres@localhost:5432/crawler_demo` |
//! | `CRAWLER_AUTHORING_MODE` | `auto` (LLM, high-confidence schemas auto-approved), `review` (LLM, human approves), `manual` (no LLM, operator writes patterns and schemas) or `agent` (opt-in: as `manual`, and the author and reviewer agents work the console with tokens generated at every start into `review-agent-tokens.json` in the working directory, mode 0600; the tokenless local console stays human); invalid values fail startup | `review` |
//! | `CRAWLER_REVIEW_REQUIRED`, `CRAWLER_REVIEW_URL_PATTERN_REQUIRED`, `CRAWLER_SCHEMA_LLM_REVIEW_MODE` | Removed: any non-empty value fails startup. Replace them with `CRAWLER_AUTHORING_MODE` (mapping in `docs/crawler/operations.md#authoring-modes`) | unset |
//! | `CRAWLER_REVIEW_BIND_ADDR` | Review UI bind address        | `127.0.0.1:7878`                                |
//! | `CRAWLER_REVIEW_AUTH_TOKEN` | Optional bearer token for the review UI/API; unset allows same-origin local mutations as a human, in `agent` mode too | unset |
//! | `CRAWLER_REVIEW_AGENT_TOKEN`, `CRAWLER_REVIEW_AGENT_APPROVALS` | Removed: any non-empty value fails startup. `agent` mode generates the agents' tokens and lets the reviewer agent decide | unset |
//! | `CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR` | The review agents' approvals plus rejections per rolling hour in `agent` mode | `20` |
//! | `CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES` | How old the live matrix or the sample labels an agent approval rests on may be | `30` |
//! | `CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS` | How many sample URLs an agent approval of a URL pattern needs labelled `PRODUCT`, and how many `NOT_PRODUCT` | `3` |
//! | `LOG_LEVEL`      | Global log level                     | `info`                                           |
//! | `CRAWLER_LOG_LEVEL` | Crawler-internal log level        | `info`                                           |
//!
//! The pipeline is wired by [`crawler::runtime`], shared with `server`. This binary only supplies
//! the demo ListingSources, short cron intervals, the tokenless local review console, and a file
//! sink: changed raw observations are written to `scraped_raw_observations.json` instead of
//! calling the ProductListing raw-capture use case. Like `server`, its working directory is its
//! state directory, where `agent` mode writes the review agents' token file; the file is
//! git-ignored.
//!
//! # Stopping
//!
//! Ctrl+C (SIGINT) or SIGTERM stops the binary gracefully: no new spider or scraper work starts,
//! running work gets 30 seconds to finish and the raw observations captured so far are written,
//! then the process exits 0. A second Ctrl+C exits at once. The contract is in
//! `docs/crawler/operations.md#stopping-the-crawler`.

use listing_source_core::ListingSourceId;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crawler::local_db::{
    DEMO_DB_NAME, bootstrap_local_database,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl, demo_db_url,
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

    async {
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

        let db_url = demo_db_url();
        if let Err(error) = bootstrap_local_database(DEMO_DB_NAME).await {
            error!(error = ?error, "Failed to bootstrap local Postgres database");
            return;
        }

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
                state_directory: crawler::runtime::state_directory(),
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
    .instrument(tracing::info_span!(
        "crawler_dev",
        entrypoint = "dev",
        database = DEMO_DB_NAME
    ))
    .await;
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// Runs `docker compose up -d` from the crawler crate directory.
///
/// `docker compose up -d` is idempotent:
/// - Container already running → no-op, returns immediately.
/// - Container exists but is stopped → restarts it.
/// - Container does not exist → creates and starts it.
///
/// The compose file path is baked in via `CARGO_MANIFEST_DIR` so this works
/// regardless of the working directory when `cargo run` is invoked.
/// Attempts to connect to Postgres, retrying with exponential back-off.
/// This handles the window between `docker compose up -d` returning and
/// Postgres actually accepting connections.
#[tracing::instrument(skip(config), fields(db_url = %db_url))]
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
    use super::{DEV_SPIDER_MAX_SIZE_BYTES, SPIDER_MAX_SIZE_BYTES_ENV, default_spider_body_limit};
    use std::env;

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
