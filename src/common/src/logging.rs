use std::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::EnvFilter;

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
    operation: &str,
    provider: &str,
    model: &str,
    latency: Duration,
    metrics: LlmInvocationMetrics,
) {
    info!(
        eventType = "llmInvocation",
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
}
