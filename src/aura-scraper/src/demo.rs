//! Demo binary — showcases end-to-end usage of [`ScraperService`].
//!
//! # What it does
//!
//! 1. Initialises structured info logging.
//! 2. Spins up a throw-away Postgres container via testcontainers.
//! 3. Executes `sql/schema.sql` against it (tables + seed state-mappings).
//! 4. Wires up all real service implementations:
//!    - [`ProductStateMappingServiceImpl`]
//!    - [`ProductNormalizationServiceImpl`]
//!    - [`ProductSchemaServiceImpl`]
//!    - [`ScraperServiceImpl`] (backed by a real [`reqwest::Client`])
//! 5. Iterates over the placeholder targets below and prints the result.
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default            |
//! |------------------|--------------------------------------|--------------------|
//! | `OPENAI_API_KEY` | API key forwarded to the LLM builder | *(required)*       |
//! | `OPENAI_MODEL`   | Model name to use                    | `gemini-2.5-flash` |
//! | `LOG_LEVEL`      | Log level for `init_logging`         | `info`             |
//!
//! # Running
//!
//! ```bash
//! OPENAI_API_KEY=sk-... cargo run --bin demo -p aura-scraper
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::process::Command;
use std::time::Duration;

use aura_scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use aura_scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use aura_scraper::normalization::product::NormalizedProduct;
use aura_scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use aura_scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use aura_scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use aura_scraper::scraper_service::{ReqwestHtmlFetcher, ScraperService, ScraperServiceImpl};
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use llm::builder::{LLMBackend, LLMBuilder};
use product::data::product_image_data::ProductImageData;
use product::data::product_state_data::ProductStateData;
use sqlx::PgPool;
use testcontainers::ImageExt;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres as PgImage;
use time::OffsetDateTime;
use tracing::{error, info};
use url::Url;

// ---------------------------------------------------------------------------
// Postgres container config
// ---------------------------------------------------------------------------

const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";
const POSTGRES_PORT: u16 = 5432;
const DEMO_CONTAINER_NAME: &str = "aura-historia-scraper-demo";

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
        }
    }
}

#[tokio::main]
async fn main() {
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
    ];

    // 1. Force info log level before init_logging reads LOG_LEVEL.
    //    Safety: single-threaded at this point, no other threads have spawned.
    unsafe { std::env::set_var("LOG_LEVEL", "info") };
    common::logging::init_logging();

    // 2. Spin up Postgres.
    let pool: &'static PgPool = start_postgres().await;

    // 3. Apply schema (tables + seed data).
    apply_schema(pool).await;

    // 4. Wire services.
    let service = build_scraper_service(pool);

    // 5. Run the scraper for each target.
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

        match service.scrape(&shop_id, &url).await {
            Ok(product) => {
                info!(
                    title = %product.title.payload,
                    shopsProductId = %product.shops_product_id,
                    "Scrape succeeded"
                );
                products.push(product.into());
            }
            Err(e) => {
                error!(error = %e, "Scrape failed");
            }
        }
    }

    // Serialize products to JSON
    info!("Scraping complete, writing output to 'scraper_output.json'…");
    let file = File::create("scraper_output.json").unwrap();
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &products).unwrap();
    info!("Output written to 'scraper_output.json'.");
}

// ---------------------------------------------------------------------------
// Postgres helpers
// ---------------------------------------------------------------------------

/// Removes any leftover demo container, starts a fresh one, waits until
/// Postgres is ready, and returns a `'static` reference to a [`PgPool`].
///
/// The pool is intentionally leaked: the repositories hold `&'static PgPool`
/// references and must outlive the service, which lives until end of `main`.
async fn start_postgres() -> &'static PgPool {
    // Clean up any container left over from a previous aborted run.
    let _ = Command::new("docker")
        .args(["rm", "-f", DEMO_CONTAINER_NAME])
        .output();

    info!("Starting Postgres container '{DEMO_CONTAINER_NAME}'…");

    let container: testcontainers::ContainerAsync<PgImage> = PgImage::default()
        .with_user(POSTGRES_USER)
        .with_password(POSTGRES_PASSWORD)
        .with_db_name(POSTGRES_DB)
        .with_container_name(DEMO_CONTAINER_NAME)
        .with_mapped_port(POSTGRES_PORT, POSTGRES_PORT.tcp())
        .start()
        .await
        .expect("failed to start Postgres container — is Docker running?");

    info!("Postgres container started.");

    // Keep the container alive for the duration of the process.
    std::mem::forget(container);

    // Register a best-effort cleanup on exit so the container is removed even
    // if the process exits without reaching the end of main.
    install_cleanup();

    let connection_string = format!(
        "postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{POSTGRES_PORT}/{POSTGRES_DB}"
    );

    // Retry briefly — the container socket may not be ready the instant
    // `start()` resolves.
    let pool = {
        let mut attempt = 0u32;
        let mut delay = Duration::from_millis(100);
        loop {
            attempt += 1;
            match PgPool::connect(&connection_string).await {
                Ok(p) => {
                    info!(attempt, "Connected to Postgres.");
                    break p;
                }
                Err(e) if attempt < 20 => {
                    info!(attempt, error = %e, "Postgres not ready yet, retrying…");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(2));
                }
                Err(e) => panic!("Could not connect to Postgres after {attempt} attempts: {e}"),
            }
        }
    };

    // Leak the pool so it obtains a `'static` lifetime that can be shared
    // across all repository impls without fighting the borrow checker.
    Box::leak(Box::new(pool))
}

/// Executes `sql/schema.sql` (relative to the workspace root) against `pool`.
async fn apply_schema(pool: &PgPool) {
    let workspace_root = env!("CARGO_WORKSPACE_DIR");
    let sql_path = std::path::Path::new(workspace_root).join("src/aura-scraper/sql/schema.sql");

    let sql = std::fs::read_to_string(&sql_path)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", sql_path.display()));

    info!(path = %sql_path.display(), "Applying schema…");

    sqlx::raw_sql(&sql)
        .execute(pool)
        .await
        .expect("failed to apply schema.sql");

    info!("Schema applied (tables + seed state-mappings).");
}

// ---------------------------------------------------------------------------
// Service wiring
// ---------------------------------------------------------------------------

/// Constructs and returns a fully wired [`ScraperServiceImpl`].
///
/// Both LLM-backed services receive a fresh [`LLMBuilder`] each — they apply
/// their own system prompts internally via their `::new` constructors.
fn build_scraper_service(pool: &'static PgPool) -> ScraperServiceImpl {
    let api_key = std::env::var("OPENAI_API_KEY")
        .expect("OPENAI_API_KEY must be set — see the module-level doc comment");
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());

    let schema_llm_builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);

    let state_llm_builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);

    // State-mapping service (DB-backed + LLM fallback).
    let state_mapping_repo = Box::new(ProductStateMappingRepositoryImpl::new(pool));
    let state_mapping_svc =
        ProductStateMappingServiceImpl::new(state_llm_builder, state_mapping_repo)
            .expect("failed to build ProductStateMappingServiceImpl");

    // Normalization service.
    let normalization_svc = ProductNormalizationServiceImpl::new(Box::new(state_mapping_svc));

    // Schema service (DB-backed + LLM creation/fix).
    let schema_repo = Box::new(ShopsProductSchemaRepositoryImpl::new(pool));
    let schema_svc = ProductSchemaServiceImpl::new(schema_llm_builder, schema_repo)
        .expect("failed to build ProductSchemaServiceImpl");

    // HTTP fetcher with a sensible timeout and user-agent.
    let http_client = reqwest::Client::builder()
        .user_agent("aura-historia-scraper-demo/1.0")
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build reqwest::Client");
    let fetcher = Box::new(ReqwestHtmlFetcher::new(http_client));

    ScraperServiceImpl::new(fetcher, Box::new(schema_svc), Box::new(normalization_svc))
}

// ---------------------------------------------------------------------------
// Container cleanup
// ---------------------------------------------------------------------------

extern "C" fn cleanup_container() {
    let _ = Command::new("docker")
        .args(["rm", "-f", DEMO_CONTAINER_NAME])
        .output();
}

fn install_cleanup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        libc::atexit(cleanup_container);
    });
}
