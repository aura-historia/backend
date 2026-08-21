use platform_observability::{LogLevel, LoggingConfig, init, init_with_directives};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEventType {
    BatchProcessing,
    EntityWrite,
    PolicyDecision,
}

impl LogEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
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

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}

/// Legacy shim. Owner: platform-observability. Remove after legacy runtime migration.
pub fn init_logging() {
    init(logging_config_from_env());
}

/// Legacy shim. Owner: platform-observability. Remove after legacy runtime migration.
pub fn init_logging_with_directives(extra_directives: &[&str]) {
    init_with_directives(logging_config_from_env(), extra_directives);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_info_when_no_log_level_for_default_logging() {
        assert_eq!(LoggingConfig::default().level(), LogLevel::Info);
    }

    #[test]
    fn should_use_debug_when_debug_log_level_for_logging() {
        assert_eq!(LogLevel::parse("DEBUG"), Some(LogLevel::Debug));
    }

    #[test]
    fn should_use_info_when_invalid_log_level_for_logging() {
        assert_eq!(LogLevel::parse("INVALID"), None);
    }
}
