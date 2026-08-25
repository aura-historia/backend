use std::time::Duration;

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
    CrawlerProductSchemaRepair,
    CrawlerProductSchemaEvaluation,
    CrawlerAvailabilityMapping,
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
            Self::CrawlerProductSchemaRepair => "CRAWLER_PRODUCT_SCHEMA_REPAIR",
            Self::CrawlerProductSchemaEvaluation => "CRAWLER_PRODUCT_SCHEMA_EVALUATION",
            Self::CrawlerAvailabilityMapping => "CRAWLER_AVAILABILITY_MAPPING",
        }
    }
}

impl std::fmt::Display for LlmOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
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
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmModel {
    Gemini25FlashLite,
    Gemini31FlashLite,
    Gemini31ProPreview,
    GeminiEmbedding2Preview0325,
    GeminiEmbedding2,
    Configured,
}

impl LlmModel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::Gemini31FlashLite => "gemini-3.1-flash-lite",
            Self::Gemini31ProPreview => "gemini-3.1-pro-preview",
            Self::GeminiEmbedding2Preview0325 => "gemini-embedding-2-preview-03-25",
            Self::GeminiEmbedding2 => "gemini-embedding-2",
            Self::Configured => "CONFIGURED",
        }
    }
}

impl std::fmt::Display for LlmModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
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
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
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

pub fn log_llm_invocation(
    operation: LlmOperation,
    provider: LlmProvider,
    model: LlmModel,
    latency: Duration,
    metrics: LlmInvocationMetrics,
) {
    log_llm_invocation_event(
        operation,
        provider,
        model,
        latency,
        metrics,
        LlmInvocationOutcome::Completed,
    );
}

pub fn log_llm_invocation_failure(
    operation: LlmOperation,
    provider: LlmProvider,
    model: LlmModel,
    latency: Duration,
    metrics: LlmInvocationMetrics,
    failure_kind: &'static str,
) {
    log_llm_invocation_event(
        operation,
        provider,
        model,
        latency,
        metrics,
        LlmInvocationOutcome::StructuredResponseFailure { failure_kind },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmInvocationOutcome {
    Completed,
    StructuredResponseFailure { failure_kind: &'static str },
}

impl LlmInvocationOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "COMPLETED",
            Self::StructuredResponseFailure { .. } => "STRUCTURED_RESPONSE_FAILURE",
        }
    }

    const fn failure_kind(self) -> Option<&'static str> {
        match self {
            Self::Completed => None,
            Self::StructuredResponseFailure { failure_kind } => Some(failure_kind),
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Completed => "Completed LLM invocation.",
            Self::StructuredResponseFailure { .. } => {
                "LLM invocation returned a structured response failure."
            }
        }
    }
}

fn log_llm_invocation_event(
    operation: LlmOperation,
    provider: LlmProvider,
    model: LlmModel,
    latency: Duration,
    metrics: LlmInvocationMetrics,
    outcome: LlmInvocationOutcome,
) {
    let latency_millis = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX);
    let failure_kind = outcome.failure_kind();
    let message = outcome.message();
    tracing::info!(
        eventType = "LLM_INVOCATION",
        llmOperation = %operation,
        llmProvider = %provider,
        llmModel = %model,
        llmOutcome = outcome.as_str(),
        llmFailureKind = failure_kind,
        llmServiceTier = metrics.service_tier.map(|tier| tier.as_str()),
        latencyMs = latency_millis,
        batchSize = metrics.batch_size,
        promptTokens = metrics.prompt_tokens,
        completionTokens = metrics.completion_tokens,
        totalTokens = metrics.total_tokens,
        cachedPromptTokens = metrics.cached_prompt_tokens,
        reasoningTokens = metrics.reasoning_tokens,
        outputDimensions = metrics.output_dimensions,
        cacheHit = metrics.cache_hit,
        "{}", message
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_llm_operation_wire_name() {
        assert_eq!(
            LlmOperation::CrawlerUrlClassification.as_str(),
            "CRAWLER_URL_CLASSIFICATION"
        );
    }

    #[test]
    fn should_preserve_fresh_schema_generation_operation_wire_name() {
        assert_eq!(
            LlmOperation::CrawlerProductSchemaFreshGeneration.as_str(),
            "CRAWLER_PRODUCT_SCHEMA_FRESH_GENERATION"
        );
    }

    #[test]
    fn should_preserve_service_tier_wire_name() {
        assert_eq!(GeminiServiceTier::Flex.as_str(), "FLEX");
    }

    #[test]
    fn should_log_configured_model_name() {
        assert_eq!(
            LlmModel::Gemini31ProPreview.as_str(),
            "gemini-3.1-pro-preview"
        );
    }

    #[test]
    fn should_preserve_structured_failure_wire_name() {
        assert_eq!(
            crate::StructuredResponseFailureKind::MaxTokens.as_str(),
            "max_tokens"
        );
    }
}
