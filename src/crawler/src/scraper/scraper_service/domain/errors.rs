use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::product_schema_service::ProductSchemaServiceError;
use crate::scraper::normalization::error::NormalizationError;
use common::shop_id::ShopId;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ScraperError {
    #[error("HTTP error while fetching '{url}': {details}")]
    HttpError {
        url: Url,
        kind: NetworkErrorKind,
        details: String,
    },

    #[error("Product URL removed while fetching '{url}': {details}")]
    ProductRemoved { url: Url, details: String },

    #[error("URL has no host: {url}")]
    NoHost { url: Url },

    #[error("Schema service error: {0}")]
    SchemaServiceError(#[from] ProductSchemaServiceError),

    #[error(
        "Schema regeneration exhausted after {attempts} attempts for '{url}' (last error: {last_error})"
    )]
    SchemaRegenerationExhausted {
        url: Url,
        attempts: u32,
        last_error: ApplySchemaError,
    },

    /// All schema-fix attempts were consumed but normalization kept failing
    /// even after the generated schema successfully applied.  The root cause
    /// is a normalization error, not an extraction failure.
    #[error("Normalization fix exhausted after {attempts} attempts for '{url}': {last_norm_error}")]
    NormalizationFixExhausted {
        url: Url,
        attempts: u32,
        last_norm_error: NormalizationError,
    },

    #[error("Normalization error: {0}")]
    NormalizationError(#[from] NormalizationError),

    #[error(
        "LLM call budget exceeded for shop '{shop_id}' while scraping '{url}' (limit={max_calls})"
    )]
    LlmBudgetExceeded {
        shop_id: ShopId,
        url: Url,
        max_calls: i64,
    },

    #[error("Scraping '{url}' is blocked pending product schema review '{review_id}'")]
    PendingSchemaReview { url: Url, review_id: uuid::Uuid },
}
