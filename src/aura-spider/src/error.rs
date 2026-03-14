use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpiderError {
    #[error("Spider error: {0}")]
    Spider(String),

    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Gemini API error: {0}")]
    Gemini(String),

    #[error("Environment variable not set: {0}")]
    EnvVar(#[from] std::env::VarError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("No product pages found: {0}")]
    NoProducts(String),
}
