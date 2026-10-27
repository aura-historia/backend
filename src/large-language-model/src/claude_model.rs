use crate::LlmModel;
use serde::Serialize;

/// Model selection for Anthropic's direct API with model-specific thinking modes.
///
/// Strings preserve provider defaults, including for unknown model IDs. Explicit
/// settings bind the model ID, thinking mode and valid effort range together.
/// See the provider's [effort](https://platform.claude.com/docs/en/build-with-claude/effort)
/// and [thinking](https://platform.claude.com/docs/en/build-with-claude/thinking) contracts.
/// Thinking and response text share the caller's `max_output_tokens` limit.
///
/// ```
/// use large_language_model::{AnthropicConfig, ClaudeEffort, ClaudeModel};
/// let config = AnthropicConfig::new(ClaudeModel::Opus55(ClaudeEffort::Medium));
/// ```
///
/// Opus 5.5 always thinks and accepts only an effort level:
///
/// ```compile_fail
/// use large_language_model::{ClaudeHaiku55Thinking, ClaudeModel, ClaudeReducedThinkingEffort};
/// let model = ClaudeModel::Opus55(
///     ClaudeHaiku55Thinking::Disabled(ClaudeReducedThinkingEffort::Low),
/// );
/// ```
///
/// Provider configurations cannot accept a different provider's model:
///
/// ```compile_fail
/// use large_language_model::{AnthropicConfig, GeminiModel, GeminiThinkingLevel};
/// let config = AnthropicConfig::new(GeminiModel::Flash35(GeminiThinkingLevel::Low));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeModel {
    ProviderDefault(String),
    Opus55(ClaudeEffort),
    Sonnet55(ClaudeSonnet55Thinking),
    Haiku55(ClaudeHaiku55Thinking),
}

impl ClaudeModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ProviderDefault(model) => model,
            Self::Opus55(_) => "claude-opus-5-5",
            Self::Sonnet55(_) => "claude-sonnet-5-5",
            Self::Haiku55(_) => "claude-haiku-5-5",
        }
    }

    pub(crate) fn effort(&self) -> Option<ClaudeEffort> {
        match self {
            Self::ProviderDefault(_) => None,
            Self::Opus55(effort)
            | Self::Sonnet55(ClaudeSonnet55Thinking::Adaptive(effort))
            | Self::Haiku55(ClaudeHaiku55Thinking::Adaptive(effort)) => Some(*effort),
            Self::Sonnet55(ClaudeSonnet55Thinking::BetweenTools(effort))
            | Self::Haiku55(ClaudeHaiku55Thinking::Disabled(effort)) => Some((*effort).into()),
        }
    }

    pub(crate) fn thinking(&self) -> Option<ClaudeThinking> {
        match self {
            Self::ProviderDefault(_) | Self::Opus55(_) => None,
            Self::Sonnet55(ClaudeSonnet55Thinking::Adaptive(_))
            | Self::Haiku55(ClaudeHaiku55Thinking::Adaptive(_)) => Some(ClaudeThinking::Adaptive),
            Self::Sonnet55(ClaudeSonnet55Thinking::BetweenTools(_)) => {
                Some(ClaudeThinking::BetweenTools)
            }
            Self::Haiku55(ClaudeHaiku55Thinking::Disabled(_)) => Some(ClaudeThinking::Disabled),
        }
    }
}

/// Map a configured Claude model ID to its invocation-log name.
pub(crate) fn log_model(model: &str) -> LlmModel {
    match model {
        "claude-sonnet-5-5" => LlmModel::ClaudeSonnet55,
        "claude-opus-5-5" => LlmModel::ClaudeOpus55,
        "claude-haiku-5-5" => LlmModel::ClaudeHaiku55,
        _ => LlmModel::Configured,
    }
}

impl From<String> for ClaudeModel {
    fn from(model: String) -> Self {
        Self::ProviderDefault(model)
    }
}

impl From<&str> for ClaudeModel {
    fn from(model: &str) -> Self {
        Self::ProviderDefault(model.to_owned())
    }
}

impl From<&String> for ClaudeModel {
    fn from(model: &String) -> Self {
        Self::from(model.as_str())
    }
}

/// All effort levels supported by adaptive thinking on Claude 5.5 models.
/// Effort is a behavioral signal, not a token budget or a guarantee of reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeEffort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

/// Valid effort levels for Sonnet's `between_tools` and Haiku's `disabled` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeReducedThinkingEffort {
    Low,
    Medium,
    High,
}

impl From<ClaudeReducedThinkingEffort> for ClaudeEffort {
    fn from(effort: ClaudeReducedThinkingEffort) -> Self {
        match effort {
            ClaudeReducedThinkingEffort::Low => Self::Low,
            ClaudeReducedThinkingEffort::Medium => Self::Medium,
            ClaudeReducedThinkingEffort::High => Self::High,
        }
    }
}

/// Sonnet 5.5 cannot disable thinking. Without tools, `BetweenTools` returns text
/// without up-front thinking. That mode only accepts effort up to `High`.
///
/// ```compile_fail
/// use large_language_model::{ClaudeEffort, ClaudeSonnet55Thinking};
/// let thinking = ClaudeSonnet55Thinking::BetweenTools(ClaudeEffort::Max);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeSonnet55Thinking {
    Adaptive(ClaudeEffort),
    BetweenTools(ClaudeReducedThinkingEffort),
}

/// Haiku 5.5 can disable thinking only at effort up to `High`.
///
/// ```compile_fail
/// use large_language_model::{ClaudeEffort, ClaudeHaiku55Thinking};
/// let thinking = ClaudeHaiku55Thinking::Disabled(ClaudeEffort::XHigh);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeHaiku55Thinking {
    Adaptive(ClaudeEffort),
    Disabled(ClaudeReducedThinkingEffort),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ClaudeThinking {
    Adaptive,
    BetweenTools,
    Disabled,
}
