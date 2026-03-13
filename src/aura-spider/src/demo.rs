//! Demo binary - showcases end-to-end usage of [`SpiderService`].
//!
//! # Configuration
//!
//! | Env var          | Purpose                 | Default      |
//! |------------------|-------------------------|--------------|
//! | `GEMINI_API_KEY` | API key for Gemini      | *(required)* |
//! | `LOG_LEVEL`      | Log level for this demo | `info`       |
//!
//! # Running
//!
//! ```bash
//! GEMINI_API_KEY=... cargo run --bin demo -p aura-spider -- https://www.christies.com/en
//! ```

use std::env;
use std::fs::File;
use std::io::BufWriter;

use aura_spider::error::SpiderError;
use aura_spider::spider_service::SpiderService;
use tracing::{Level, error, info};

const DEFAULT_TARGET_URL: &str = "https://www.christies.com/en";
const DEFAULT_CLASSIFY_THRESHOLD: usize = 200;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    let target_url = read_target_url();
    let api_key = match read_api_key() {
        Ok(api_key) => api_key,
        Err(error) => {
            error!(error = %error, "Failed to load configuration");
            return;
        }
    };

    let spider = SpiderService::new(target_url, api_key, DEFAULT_CLASSIFY_THRESHOLD);

    match spider.run().await {
        Ok(products) => {
            info!(count = products.len(), "Spider run finished successfully");
            if let Err(error) = write_output(&products) {
                error!(error = %error, "Failed to write demo output file");
            } else {
                info!("Output written to 'spider_output.json'");
            }
        }
        Err(error) => {
            error!(error = %error, "Spider run failed");
        }
    }
}

fn read_target_url() -> String {
    let raw_url = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_TARGET_URL.to_string());

    ensure_scheme(&raw_url)
}

fn read_api_key() -> Result<String, SpiderError> {
    Ok(env::var("GEMINI_API_KEY")?)
}

fn ensure_scheme(url: &str) -> String {
    let trimmed = url.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let level = raw_level.parse::<Level>().unwrap_or(Level::INFO);

    tracing_subscriber::fmt().with_max_level(level).init();
}

fn write_output(products: &[String]) -> Result<(), std::io::Error> {
    let file = File::create("spider_output.json")?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, products)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}
