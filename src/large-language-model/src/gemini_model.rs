use serde::Serialize;

/// Model selection for Vertex AI Gemini, including model-specific thinking levels.
///
/// Strings select a model with provider defaults. Explicit thinking is available
/// only through the named variants, so an unknown model cannot inherit another
/// model's capabilities. Levels follow the provider's
/// [capability table](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/thinking#thinking-level-values).
/// `Minimal` minimizes thinking; it does not guarantee that thinking is disabled.
///
/// ```
/// use large_language_model::{GeminiModel, GeminiThinkingLevel, VertexAiConfig};
/// let config = VertexAiConfig::new(
///     "project", "global", GeminiModel::Flash35(GeminiThinkingLevel::Low),
/// );
/// ```
///
/// Pro does not support the Flash models' `Minimal` level:
///
/// ```compile_fail
/// use large_language_model::{GeminiModel, GeminiThinkingLevel};
/// let model = GeminiModel::Pro31Preview(GeminiThinkingLevel::Minimal);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiModel {
    ProviderDefault(String),
    FlashLite31(GeminiThinkingLevel),
    Pro31Preview(GeminiProThinkingLevel),
    Flash35(GeminiThinkingLevel),
}

impl GeminiModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ProviderDefault(model) => model,
            Self::FlashLite31(_) => "gemini-3.1-flash-lite",
            Self::Pro31Preview(_) => "gemini-3.1-pro-preview",
            Self::Flash35(_) => "gemini-3.5-flash",
        }
    }

    pub(crate) fn thinking_config(&self) -> Option<GeminiThinkingConfig> {
        let thinking_level = match self {
            Self::ProviderDefault(_) => return None,
            Self::FlashLite31(level) | Self::Flash35(level) => *level,
            Self::Pro31Preview(level) => match level {
                GeminiProThinkingLevel::Low => GeminiThinkingLevel::Low,
                GeminiProThinkingLevel::Medium => GeminiThinkingLevel::Medium,
                GeminiProThinkingLevel::High => GeminiThinkingLevel::High,
            },
        };
        Some(GeminiThinkingConfig { thinking_level })
    }
}

impl From<String> for GeminiModel {
    fn from(model: String) -> Self {
        Self::ProviderDefault(model)
    }
}

impl From<&str> for GeminiModel {
    fn from(model: &str) -> Self {
        Self::ProviderDefault(model.to_owned())
    }
}

impl From<&String> for GeminiModel {
    fn from(model: &String) -> Self {
        Self::from(model.as_str())
    }
}

/// Thinking levels supported by Gemini 3.1 Flash-Lite and Gemini 3.5 Flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GeminiThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

/// Thinking levels supported by Gemini 3.1 Pro Preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiProThinkingLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiThinkingConfig {
    thinking_level: GeminiThinkingLevel,
}
