use std::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEventType {
    LlmInvocation,
    BatchProcessing,
    EntityWrite,
    PolicyDecision,
    ClassificationDecision,
}

impl LogEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LlmInvocation => "LLM_INVOCATION",
            Self::BatchProcessing => "BATCH_PROCESSING",
            Self::EntityWrite => "ENTITY_WRITE",
            Self::PolicyDecision => "POLICY_DECISION",
            Self::ClassificationDecision => "CLASSIFICATION_DECISION",
        }
    }
}

impl std::fmt::Display for LogEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEntityType {
    Product,
}

impl LogEntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Product => "product",
        }
    }
}

impl std::fmt::Display for LogEntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPipelineStage {
    ProductClassification,
    ProductTranslation,
    ProductAttributeExtraction,
    ProductEmbedding,
}

impl LogPipelineStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductClassification => "PRODUCT_CLASSIFICATION",
            Self::ProductTranslation => "PRODUCT_TRANSLATION",
            Self::ProductAttributeExtraction => "PRODUCT_ATTRIBUTE_EXTRACTION",
            Self::ProductEmbedding => "PRODUCT_EMBEDDING",
        }
    }
}

impl std::fmt::Display for LogPipelineStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogWriteSource {
    ProductCommandService,
    ProductClassification,
    ProductTranslation,
    ProductAttributeExtraction,
    ProductEmbedding,
}

impl LogWriteSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductCommandService => "PRODUCT_COMMAND_SERVICE",
            Self::ProductClassification => "PRODUCT_CLASSIFICATION",
            Self::ProductTranslation => "PRODUCT_TRANSLATION",
            Self::ProductAttributeExtraction => "PRODUCT_ATTRIBUTE_EXTRACTION",
            Self::ProductEmbedding => "PRODUCT_EMBEDDING",
        }
    }
}

impl std::fmt::Display for LogWriteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogClassificationMethod {
    ClearScore,
    Llm,
}

impl LogClassificationMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClearScore => "CLEAR_SCORE",
            Self::Llm => "LLM",
        }
    }
}

impl std::fmt::Display for LogClassificationMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmOperation {
    ProductClassification,
    ProductTitleTranslation,
    ProductAttributeExtraction,
    ProductEmbedding,
    ProductQueryEmbedding,
    SellerShopDisambiguation,
    CrawlerUrlClassification,
    CrawlerProductSchemaGeneration,
    CrawlerProductSchemaRepair,
    CrawlerProductStateMapping,
}

impl LlmOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductClassification => "PRODUCT_CLASSIFICATION",
            Self::ProductTitleTranslation => "PRODUCT_TITLE_TRANSLATION",
            Self::ProductAttributeExtraction => "PRODUCT_ATTRIBUTE_EXTRACTION",
            Self::ProductEmbedding => "PRODUCT_EMBEDDING",
            Self::ProductQueryEmbedding => "PRODUCT_QUERY_EMBEDDING",
            Self::SellerShopDisambiguation => "SELLER_SHOP_DISAMBIGUATION",
            Self::CrawlerUrlClassification => "CRAWLER_URL_CLASSIFICATION",
            Self::CrawlerProductSchemaGeneration => "CRAWLER_PRODUCT_SCHEMA_GENERATION",
            Self::CrawlerProductSchemaRepair => "CRAWLER_PRODUCT_SCHEMA_REPAIR",
            Self::CrawlerProductStateMapping => "CRAWLER_PRODUCT_STATE_MAPPING",
        }
    }
}

impl std::fmt::Display for LlmOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    Google,
    Configured,
}

impl LlmProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Google => "GOOGLE",
            Self::Configured => "CONFIGURED",
        }
    }
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmModel {
    Gemini25FlashLite,
    GeminiEmbedding2Preview0325,
    Configured,
}

impl LlmModel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::GeminiEmbedding2Preview0325 => "gemini-embedding-2-preview-03-25",
            Self::Configured => "CONFIGURED",
        }
    }
}

impl std::fmt::Display for LlmModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LlmInvocationMetrics {
    pub batch_size: Option<usize>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub cached_prompt_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub output_dimensions: Option<usize>,
    pub cache_hit: Option<bool>,
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn parse_log_level(log_level: &str) -> Option<Level> {
    match log_level.to_ascii_uppercase().as_str() {
        "TRACE" => Some(Level::TRACE),
        "DEBUG" => Some(Level::DEBUG),
        "INFO" => Some(Level::INFO),
        "WARN" => Some(Level::WARN),
        "ERROR" => Some(Level::ERROR),
        _ => None,
    }
}

fn resolve_log_level(log_level: Option<&str>) -> Level {
    log_level
        .and_then(parse_log_level)
        .unwrap_or(tracing::Level::INFO)
}

pub fn init_logging() {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let log_level = resolve_log_level(configured_log_level.as_deref());

    tracing_subscriber::fmt()
        .json()
        .with_max_level(log_level)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    tracing::debug!(log_level = ?log_level, "Logger initialized.");
}

pub fn log_llm_invocation(
    operation: LlmOperation,
    provider: LlmProvider,
    model: LlmModel,
    latency: Duration,
    metrics: LlmInvocationMetrics,
) {
    log_llm_invocation_with_context(
        operation.as_str(),
        provider.as_str(),
        model.as_str(),
        latency,
        metrics,
        None,
        None,
    );
}

pub fn log_llm_invocation_with_context(
    operation: &str,
    provider: &str,
    model: &str,
    latency: Duration,
    metrics: LlmInvocationMetrics,
    service: Option<&str>,
    component: Option<&str>,
) {
    match (service, component) {
        (Some(service), Some(component)) => info!(
            service,
            component,
            eventType = %LogEventType::LlmInvocation,
            llmOperation = operation,
            llmProvider = provider,
            llmModel = model,
            latencyMs = duration_millis(latency),
            batchSize = metrics.batch_size,
            promptTokens = metrics.prompt_tokens,
            completionTokens = metrics.completion_tokens,
            totalTokens = metrics.total_tokens,
            cachedPromptTokens = metrics.cached_prompt_tokens,
            reasoningTokens = metrics.reasoning_tokens,
            outputDimensions = metrics.output_dimensions,
            cacheHit = metrics.cache_hit,
            "Completed LLM invocation."
        ),
        (Some(service), None) => info!(
            service,
            eventType = %LogEventType::LlmInvocation,
            llmOperation = operation,
            llmProvider = provider,
            llmModel = model,
            latencyMs = duration_millis(latency),
            batchSize = metrics.batch_size,
            promptTokens = metrics.prompt_tokens,
            completionTokens = metrics.completion_tokens,
            totalTokens = metrics.total_tokens,
            cachedPromptTokens = metrics.cached_prompt_tokens,
            reasoningTokens = metrics.reasoning_tokens,
            outputDimensions = metrics.output_dimensions,
            cacheHit = metrics.cache_hit,
            "Completed LLM invocation."
        ),
        (None, Some(component)) => info!(
            component,
            eventType = %LogEventType::LlmInvocation,
            llmOperation = operation,
            llmProvider = provider,
            llmModel = model,
            latencyMs = duration_millis(latency),
            batchSize = metrics.batch_size,
            promptTokens = metrics.prompt_tokens,
            completionTokens = metrics.completion_tokens,
            totalTokens = metrics.total_tokens,
            cachedPromptTokens = metrics.cached_prompt_tokens,
            reasoningTokens = metrics.reasoning_tokens,
            outputDimensions = metrics.output_dimensions,
            cacheHit = metrics.cache_hit,
            "Completed LLM invocation."
        ),
        (None, None) => info!(
            eventType = %LogEventType::LlmInvocation,
            llmOperation = operation,
            llmProvider = provider,
            llmModel = model,
            latencyMs = duration_millis(latency),
            batchSize = metrics.batch_size,
            promptTokens = metrics.prompt_tokens,
            completionTokens = metrics.completion_tokens,
            totalTokens = metrics.total_tokens,
            cachedPromptTokens = metrics.cached_prompt_tokens,
            reasoningTokens = metrics.reasoning_tokens,
            outputDimensions = metrics.output_dimensions,
            cacheHit = metrics.cache_hit,
            "Completed LLM invocation."
        ),
    }
}

/// Like [`init_logging`] but accepts additional [`EnvFilter`] directives that
/// are appended after the global log level.
///
/// # Example
///
/// ```rust
/// // Suppress noisy third-party crates while keeping your own logs at INFO.
/// common::logging::init_logging_with_directives(&["spider=warn", "sqlx::postgres::notice=warn"]);
/// ```
pub fn init_logging_with_directives(extra_directives: &[&str]) {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let raw_level = configured_log_level
        .as_deref()
        .unwrap_or("info")
        .to_string();

    let directives = if extra_directives.is_empty() {
        raw_level
    } else {
        format!("{},{}", raw_level, extra_directives.join(","))
    };

    let filter = EnvFilter::new(directives);

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    tracing::debug!("Logger initialized with extra directives.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_info_when_no_log_level_for_default_logging() {
        assert_eq!(resolve_log_level(None), Level::INFO);
    }

    #[test]
    fn should_use_debug_when_debug_log_level_for_logging() {
        assert_eq!(resolve_log_level(Some("DEBUG")), Level::DEBUG);
    }

    #[test]
    fn should_use_info_when_invalid_log_level_for_logging() {
        assert_eq!(resolve_log_level(Some("INVALID")), Level::INFO);
    }

    #[test]
    fn should_convert_duration_to_millis_for_logging() {
        assert_eq!(duration_millis(Duration::from_millis(42)), 42);
    }

    #[test]
    fn should_return_crawler_url_classification_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerUrlClassification.as_str(),
            "CRAWLER_URL_CLASSIFICATION"
        );
    }

    #[test]
    fn should_return_crawler_product_schema_generation_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductSchemaGeneration.as_str(),
            "CRAWLER_PRODUCT_SCHEMA_GENERATION"
        );
    }

    #[test]
    fn should_return_crawler_product_schema_repair_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductSchemaRepair.as_str(),
            "CRAWLER_PRODUCT_SCHEMA_REPAIR"
        );
    }

    #[test]
    fn should_return_crawler_product_state_mapping_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductStateMapping.as_str(),
            "CRAWLER_PRODUCT_STATE_MAPPING"
        );
    }
}
