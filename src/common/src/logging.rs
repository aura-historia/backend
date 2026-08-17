use std::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEventType {
    LlmInvocation,
    BatchProcessing,
    EntityWrite,
    PolicyDecision,
}

impl LogEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LlmInvocation => "LLM_INVOCATION",
            Self::BatchProcessing => "BATCH_PROCESSING",
            Self::EntityWrite => "ENTITY_WRITE",
            Self::PolicyDecision => "POLICY_DECISION",
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
    ProductTranslation,
    ProductEmbedding,
}

impl LogPipelineStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductTranslation => "PRODUCT_TRANSLATION",
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
    ProductTranslation,
    ProductEmbedding,
}

impl LogWriteSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductCommandService => "PRODUCT_COMMAND_SERVICE",
            Self::ProductTranslation => "PRODUCT_TRANSLATION",
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
pub enum LogProductCommandIntent {
    Create,
    Update,
    Upsert,
}

impl LogProductCommandIntent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Update => "UPDATE",
            Self::Upsert => "UPSERT",
        }
    }
}

impl std::fmt::Display for LogProductCommandIntent {
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
    ProductTitleTranslation,
    ProductEmbedding,
    ProductQueryEmbedding,
    ProductEnhancedSearchDescriptionMatching,
    SellerShopDisambiguation,
    CrawlerUrlClassification,
    CrawlerProductSchemaGeneration,
    CrawlerProductSchemaFreshGeneration,
    CrawlerProductSchemaEvaluation,
    CrawlerProductStateMapping,
}

impl LlmOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductTitleTranslation => "PRODUCT_TITLE_TRANSLATION",
            Self::ProductEmbedding => "PRODUCT_EMBEDDING",
            Self::ProductQueryEmbedding => "PRODUCT_QUERY_EMBEDDING",
            Self::ProductEnhancedSearchDescriptionMatching => {
                "PRODUCT_ENHANCED_SEARCH_DESCRIPTION_MATCHING"
            }
            Self::SellerShopDisambiguation => "SELLER_SHOP_DISAMBIGUATION",
            Self::CrawlerUrlClassification => "CRAWLER_URL_CLASSIFICATION",
            Self::CrawlerProductSchemaGeneration => "CRAWLER_PRODUCT_SCHEMA_GENERATION",
            Self::CrawlerProductSchemaFreshGeneration => "CRAWLER_PRODUCT_SCHEMA_FRESH_GENERATION",
            Self::CrawlerProductSchemaEvaluation => "CRAWLER_PRODUCT_SCHEMA_EVALUATION",
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
    Gemini31FlashLite,
    GeminiEmbedding2Preview0325,
    GeminiEmbedding2,
    Configured,
}

impl LlmModel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::Gemini31FlashLite => "gemini-3.1-flash-lite",
            Self::GeminiEmbedding2Preview0325 => "gemini-embedding-2-preview-03-25",
            Self::GeminiEmbedding2 => "gemini-embedding-2",
            Self::Configured => "CONFIGURED",
        }
    }
}

impl std::fmt::Display for LlmModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiServiceTier {
    Standard,
    Flex,
}

impl GeminiServiceTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "STANDARD",
            Self::Flex => "FLEX",
        }
    }
}

impl std::fmt::Display for GeminiServiceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LlmInvocationMetrics {
    pub service_tier: Option<GeminiServiceTier>,
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
    info!(
        eventType = %LogEventType::LlmInvocation,
        llmOperation = %operation,
        llmProvider = %provider,
        llmModel = %model,
        llmServiceTier = metrics.service_tier.map(|tier| tier.as_str()),
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
    );
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
    use serde_json::Value;
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing::Dispatch;
    use tracing_subscriber::fmt::writer::MakeWriter;

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
    fn should_return_crawler_product_schema_fresh_generation_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductSchemaFreshGeneration.as_str(),
            "CRAWLER_PRODUCT_SCHEMA_FRESH_GENERATION"
        );
    }

    #[test]
    fn should_return_crawler_product_schema_evaluation_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductSchemaEvaluation.as_str(),
            "CRAWLER_PRODUCT_SCHEMA_EVALUATION"
        );
    }

    #[test]
    fn should_return_crawler_product_state_mapping_name_for_llm_operation() {
        assert_eq!(
            LlmOperation::CrawlerProductStateMapping.as_str(),
            "CRAWLER_PRODUCT_STATE_MAPPING"
        );
    }

    #[test]
    fn should_include_current_span_fields_when_logging_llm_invocation() {
        #[derive(Clone, Default)]
        struct SharedWriter {
            output: Arc<Mutex<Vec<u8>>>,
        }

        struct GuardedWriter {
            output: Arc<Mutex<Vec<u8>>>,
        }

        impl<'a> MakeWriter<'a> for SharedWriter {
            type Writer = GuardedWriter;

            fn make_writer(&'a self) -> Self::Writer {
                GuardedWriter {
                    output: Arc::clone(&self.output),
                }
            }
        }

        impl io::Write for GuardedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.output
                    .lock()
                    .expect("log capture mutex poisoned")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let writer = SharedWriter::default();
        let captured = Arc::clone(&writer.output);
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_ansi(false)
            .with_writer(writer)
            .finish();

        let dispatch = Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            let span = tracing::info_span!(
                "crawler_llm",
                shop_url = "https://example.com",
                url_count = 3
            );
            let _guard = span.enter();
            log_llm_invocation(
                LlmOperation::CrawlerUrlClassification,
                LlmProvider::Google,
                LlmModel::Configured,
                Duration::from_millis(42),
                LlmInvocationMetrics {
                    service_tier: Some(GeminiServiceTier::Flex),
                    batch_size: Some(3),
                    prompt_tokens: Some(12),
                    completion_tokens: Some(4),
                    total_tokens: Some(16),
                    cached_prompt_tokens: Some(2),
                    reasoning_tokens: Some(1),
                    output_dimensions: Some(5),
                    cache_hit: Some(true),
                },
            );
        });

        let output =
            String::from_utf8(captured.lock().expect("log capture mutex poisoned").clone())
                .expect("captured logs must be utf-8");
        let event: Value = serde_json::from_str(
            output
                .lines()
                .find(|line| !line.trim().is_empty())
                .expect("expected one log line"),
        )
        .expect("log output must be valid json");

        assert_eq!(event["fields"]["eventType"], "LLM_INVOCATION");
        assert_eq!(
            event["fields"]["llmOperation"],
            "CRAWLER_URL_CLASSIFICATION"
        );
        assert_eq!(event["fields"]["llmServiceTier"], "FLEX");
        assert_eq!(event["fields"]["latencyMs"], 42);
        assert_eq!(event["span"]["shop_url"], "https://example.com");
        assert_eq!(event["span"]["url_count"], 3);
    }

    #[test]
    fn should_include_null_service_tier_when_not_provided_for_llm_invocation() {
        #[derive(Clone, Default)]
        struct SharedWriter {
            output: Arc<Mutex<Vec<u8>>>,
        }

        struct GuardedWriter {
            output: Arc<Mutex<Vec<u8>>>,
        }

        impl<'a> MakeWriter<'a> for SharedWriter {
            type Writer = GuardedWriter;

            fn make_writer(&'a self) -> Self::Writer {
                GuardedWriter {
                    output: Arc::clone(&self.output),
                }
            }
        }

        impl io::Write for GuardedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.output
                    .lock()
                    .expect("log capture mutex poisoned")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let writer = SharedWriter::default();
        let captured = Arc::clone(&writer.output);
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_ansi(false)
            .with_writer(writer)
            .finish();

        let dispatch = Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            log_llm_invocation(
                LlmOperation::ProductTitleTranslation,
                LlmProvider::Google,
                LlmModel::Gemini25FlashLite,
                Duration::from_millis(7),
                LlmInvocationMetrics::default(),
            );
        });

        let output =
            String::from_utf8(captured.lock().expect("log capture mutex poisoned").clone())
                .expect("captured logs must be utf-8");
        let event: Value = serde_json::from_str(
            output
                .lines()
                .find(|line| !line.trim().is_empty())
                .expect("expected one log line"),
        )
        .expect("log output must be valid json");

        assert!(event["fields"]["llmServiceTier"].is_null());
    }
}
