//! Demo binary — showcases end-to-end usage of [`ScraperService`].
//!
//! Uses a hardcoded local Postgres database (`crawler_demo_scraper`). Bootstrap with:
//!
//! ```powershell
//! # from src/crawler/
//! .\db-up.ps1
//! .\db-migrate.ps1
//! ```
//!
//! # What it does
//!
//! 1. Initialises structured info logging.
//! 2. Connects to local Postgres and applies pending migrations.
//! 3. Wires up all real service implementations:
//!    - [`ProductStateMappingServiceImpl`]
//!    - [`ProductNormalizationServiceImpl`]
//!    - [`ProductSchemaServiceImpl`]
//!    - [`ScraperServiceImpl`] (backed by a real [`reqwest::Client`])
//! 4. Iterates over the placeholder targets below and writes results to JSON.
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default                         |
//! |------------------|--------------------------------------|--------------------|
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL               | `.../crawler_demo_scraper` |
//! | `GEMINI_API_KEY` | API key forwarded to the LLM builder | *(required)*       |
//! | `GEMINI_MODEL`   | Model name to use                    | `gemini-3.1-flash-lite-preview` |
//! | `GEMINI_CHEAP_MODEL` | Default cheaper model for low-risk LLM calls | `gemini-2.5-flash-lite` |
//! | `GEMINI_STATE_MAPPING_MODEL` | Optional override for state mapping calls | `GEMINI_CHEAP_MODEL` |
//! | `GEMINI_FLEX`    | Enable Gemini Flex inference         | unset / `false` |
//! | `LOG_LEVEL`      | Log level for `init_logging`         | `info`             |
//!
//! # Running
//!
//! ```powershell
//! $env:GEMINI_API_KEY="sk-..."
//! $env:GEMINI_FLEX="true" # optional
//! cargo run --bin demo-scraper -p crawler
//! ```

use common::shop_id::ShopId;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

use common::language::data::LocalizedTextData;
use common::logging::GeminiServiceTier;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use crawler::google_llm::{
    GeminiRateLimitConfig, GeminiRateLimiter, gemini_flex_enabled, google_llm_builder,
    state_mapping_gemini_model,
};
use crawler::local_db::{DEMO_SCRAPER_DB_NAME, bootstrap_local_database, demo_scraper_db_url};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product::NormalizedProduct;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, ReqwestHtmlFetcher, ScraperService, ScraperServiceImpl,
};
use product::data::product_image_data::ProductImageData;
use product::data::product_state_data::ProductStateData;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;
use tracing::{Instrument, error, info};
use url::Url;

// ---------------------------------------------------------------------------
// Pool sizing
// ---------------------------------------------------------------------------

/// Scraper demo pool size: 5 connections cover all repository queries with room to spare.
const DEMO_POOL_MAX_CONNECTIONS: u32 = 5;

// ---------------------------------------------------------------------------
// Scrape targets — fill in your own shop IDs and URLs below
// ---------------------------------------------------------------------------

struct ScrapeTarget {
    shop_id: ShopId,
    url: &'static str,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct DemoProduct {
    pub shops_product_id: ShopsProductId,
    pub title: LocalizedTextData,
    pub description: Option<LocalizedTextData>,
    pub price: Option<PriceData>,
    pub price_estimate_min: Option<PriceData>,
    pub price_estimate_max: Option<PriceData>,
    pub state: ProductStateData,
    pub url: Url,
    pub images: Vec<ProductImageData>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}

impl From<NormalizedProduct> for DemoProduct {
    fn from(p: NormalizedProduct) -> Self {
        Self {
            shops_product_id: p.shops_product_id,
            title: p.title.into(),
            description: p.description.map(Into::into),
            price: p.price.map(Into::into),
            price_estimate_min: p.price_estimate_min.map(Into::into),
            price_estimate_max: p.price_estimate_max.map(Into::into),
            state: p.state.into(),
            url: p.url,
            images: p.images.into_iter().map(Into::into).collect(),
            auction_start: p.auction_start,
            auction_end: p.auction_end,
            raw_attributes: p.raw_attributes,
        }
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let targets: &[ScrapeTarget] = &[
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5a".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/weichholzschrank-mit-orig-bemalung-salzburg-um-1800-art-7001/",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5a".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/bildnis-in-oel-der-gattin-von-samuel-de-la-roche1947-art-g1475/",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5b".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=51609",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5b".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=52012",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5b".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=52014",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5a".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/https-www-antiquitaeten-tuebingen-de-gemaelde-artnr-g-58-oelgemaelde-landschaftsmalerei-mitte-19-jh/",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5c".try_into().unwrap(),
            url: "https://nostalgie-palast.de/couchtisch-uebersee-mit-glasplatte-113-m-x-053-m/",
        },
        ScrapeTarget {
            shop_id: "8ded4706-dc72-4b0b-9357-9192e18e3d5d".try_into().unwrap(),
            url: "https://www.lot-tissimo.com/de-de/auction-catalogues/chiswick-auctions/catalogue-id-srchis11168/lot-61a5b754-6fc7-435b-80b3-b3fa0141c94e",
        },
    ];

    unsafe { std::env::set_var("LOG_LEVEL", "info") };
    common::logging::init_logging_with_directives(&[HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE]);

    async {
        let pool: &'static PgPool = connect_and_migrate().await;
        let service = build_scraper_service(pool);

        let mut products: Vec<DemoProduct> = vec![];
        for target in targets {
            let shop_id = target.shop_id;
            let url = match Url::parse(target.url) {
                Ok(u) => u,
                Err(e) => {
                    error!(
                        url = target.url,
                        error = %e,
                        "Invalid URL — skipping"
                    );
                    continue;
                }
            };

            let scrape_span = tracing::info_span!(
                "scraper_demo_target",
                shop_id = %shop_id,
                url = %url
            );
            match service
                .scrape(&shop_id, &url, None, None)
                .instrument(scrape_span)
                .await
            {
                Ok(Some(scraped)) => {
                    info!(
                        title = %scraped.product.title.payload,
                        shopsProductId = %scraped.product.shops_product_id,
                        "Scrape succeeded"
                    );
                    products.push(scraped.product.into());
                }
                Ok(None) => {
                    info!("Hash matched, skipped scraping");
                }
                Err(e) => {
                    error!(error = %e, "Scrape failed");
                }
            }
        }

        info!("Scraping complete, writing output to 'scraper_output.json'…");
        let file = File::create("scraper_output.json").unwrap();
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &products).unwrap();
        info!("Output written to 'scraper_output.json'.");
    }
    .instrument(tracing::info_span!(
        "crawler_scraper_demo",
        entrypoint = "demo-scraper",
        database = DEMO_SCRAPER_DB_NAME,
        target_count = targets.len()
    ))
    .await;
}

// ---------------------------------------------------------------------------
// Postgres helpers
// ---------------------------------------------------------------------------

/// Connects to local Postgres, applies pending migrations, and
/// returns a `'static` reference to a [`PgPool`].
///
/// The pool is intentionally leaked: the repositories hold `&'static PgPool`
/// references and must outlive the service, which lives until end of `main`.
#[tracing::instrument]
async fn connect_and_migrate() -> &'static PgPool {
    bootstrap_local_database(DEMO_SCRAPER_DB_NAME)
        .await
        .expect("Failed to bootstrap local Postgres database");
    let db_url = demo_scraper_db_url();

    let pool = PgPoolOptions::new()
        .max_connections(DEMO_POOL_MAX_CONNECTIONS)
        .connect(&db_url)
        .await
        .expect("Failed to connect to Postgres");

    info!(
        max_connections = DEMO_POOL_MAX_CONNECTIONS,
        "Connected to Postgres"
    );

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to apply database migrations");

    info!("Database migrations applied successfully");

    // Leak the pool so it obtains a `'static` lifetime that can be shared
    // across all repository impls without fighting the borrow checker.
    Box::leak(Box::new(pool))
}

// ---------------------------------------------------------------------------
// Service wiring
// ---------------------------------------------------------------------------

/// Constructs and returns a fully wired [`ScraperServiceImpl`].
///
/// Both LLM-backed services receive a fresh [`LLMBuilder`] each — they apply
/// their own system prompts internally via their `::new` constructors.
#[tracing::instrument(skip(pool))]
fn build_scraper_service(pool: &'static PgPool) -> ScraperServiceImpl {
    let api_key = std::env::var("GEMINI_API_KEY")
        .expect("GEMINI_API_KEY must be set — see the module-level doc comment");
    let model = std::env::var("GEMINI_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-flash-lite-preview".to_string());
    let state_model = state_mapping_gemini_model();
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

    info!(
        gemini_model = %model,
        gemini_state_mapping_model = %state_model,
        gemini_service_tier,
        "Crawler scraper demo Gemini configuration resolved"
    );
    let gemini_rate_limiter = Arc::new(GeminiRateLimiter::new(GeminiRateLimitConfig::from_env()));

    let create_schema_llm_builder = google_llm_builder(&api_key, &model, gemini_flex);
    let single_schema_llm_builder = google_llm_builder(&api_key, &model, gemini_flex);

    let state_llm_builder = google_llm_builder(&api_key, &state_model, gemini_flex);

    // State-mapping service (DB-backed + LLM fallback).
    let state_mapping_repo = Box::new(ProductStateMappingRepositoryImpl::new(pool));
    let state_mapping_svc = ProductStateMappingServiceImpl::new(
        state_llm_builder,
        llm_service_tier,
        state_mapping_repo,
        Some(Arc::clone(&gemini_rate_limiter)),
    )
    .expect("failed to build ProductStateMappingServiceImpl");

    // Normalization service.
    let normalization_svc = ProductNormalizationServiceImpl::new(Box::new(state_mapping_svc));

    // Schema service (DB-backed + LLM creation/fix).
    let schema_repo = Box::new(ShopsProductSchemaRepositoryImpl::new(pool));
    let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(pool));
    let schema_svc = ProductSchemaServiceImpl::new(
        create_schema_llm_builder,
        single_schema_llm_builder,
        llm_service_tier,
        schema_repo,
        Some(Arc::clone(&gemini_rate_limiter)),
    )
    .expect("failed to build ProductSchemaServiceImpl");

    // HTTP fetcher using spider.
    let fetcher = Box::new(ReqwestHtmlFetcher::new());

    let candidate_service = Arc::new(ScraperCandidateServiceImpl::new(pool.clone()));

    ScraperServiceImpl::new_with_schema_seed_pages(
        fetcher,
        Box::new(schema_svc),
        Box::new(normalization_svc),
        candidate_service,
        3,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    )
    .with_removed_page_schema_repository(removed_page_schema_repo)
}
