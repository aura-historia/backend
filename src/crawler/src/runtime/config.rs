//! Environment settings shared by every crawler entry point.

use super::shutdown::DEFAULT_SHUTDOWN_GRACE_PERIOD;
use crate::authoring::CrawlerAuthoringMode;
use crate::llm_runtime::CrawlerLlmRateLimitConfig;
use crate::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiConfigError, CrawlerVertexAiModels};
use std::time::Duration;
use tracing::info;

const AUTHORING_MODE_ENV: &str = "CRAWLER_AUTHORING_MODE";

/// What replaced the review settings `CRAWLER_AUTHORING_MODE` took over.
const SET_AUTHORING_MODE_INSTEAD: &str =
    "set CRAWLER_AUTHORING_MODE (auto, review or manual) instead";

/// Removed settings and what replaced each. A value in one of them fails startup, so an
/// environment cannot keep a dead setting and believe it still applies.
const REMOVED_ENV: [(&str, &str); 3] = [
    ("CRAWLER_REVIEW_REQUIRED", SET_AUTHORING_MODE_INSTEAD),
    (
        "CRAWLER_REVIEW_URL_PATTERN_REQUIRED",
        SET_AUTHORING_MODE_INSTEAD,
    ),
    ("CRAWLER_SCHEMA_LLM_REVIEW_MODE", SET_AUTHORING_MODE_INSTEAD),
];

/// Environment settings shared by every crawler entry point.
pub struct CrawlerRuntimeConfig {
    pub authoring: CrawlerAuthoringMode,
    pub llm: CrawlerLlmConfig,
    /// How long running work may take to finish after the first SIGTERM or SIGINT.
    pub shutdown_grace_period: Duration,
}

/// Whether the crawler may call an LLM, and with which provider settings.
pub enum CrawlerLlmConfig {
    /// The LLM generates URL patterns and product schemas.
    VertexAi(CrawlerVertexAiLlmConfig),
    /// No LLM calls: operators author URL patterns and product schemas in the review console.
    Disabled,
}

pub struct CrawlerVertexAiLlmConfig {
    pub vertex_ai: CrawlerVertexAiConfig,
    pub models: CrawlerVertexAiModels,
    pub rate_limit: CrawlerLlmRateLimitConfig,
}

/// Where an effective setting came from, for the startup log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Environment,
    Default,
}

impl ConfigSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "env",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerRuntimeConfigError {
    #[error("invalid {name} value {value:?}; expected one of: {accepted}")]
    InvalidValue {
        name: &'static str,
        value: String,
        accepted: String,
    },
    #[error("{name} was removed; {replacement}")]
    RemovedSetting {
        name: &'static str,
        replacement: &'static str,
    },
    #[error(transparent)]
    VertexAi(#[from] CrawlerVertexAiConfigError),
}

impl CrawlerRuntimeConfig {
    /// Reads the authoring mode and, unless it is `manual`, the Vertex AI selection and LLM
    /// rate limits, then logs the effective configuration that [`build`](super::build) applies.
    /// The shutdown grace period is no environment setting; it starts at
    /// [`DEFAULT_SHUTDOWN_GRACE_PERIOD`].
    pub fn from_env() -> Result<Self, CrawlerRuntimeConfigError> {
        for (name, replacement) in REMOVED_ENV {
            // A non-UTF-8 value is still a value: it must fail like any other.
            if !matches!(env_value(name), Ok(None)) {
                return Err(CrawlerRuntimeConfigError::RemovedSetting { name, replacement });
            }
        }
        let (authoring, source) = enum_from_env(
            AUTHORING_MODE_ENV,
            CrawlerAuthoringMode::parse,
            &CrawlerAuthoringMode::ALL.map(CrawlerAuthoringMode::as_str),
        )?;
        let llm = if authoring.uses_llm() {
            CrawlerLlmConfig::VertexAi(CrawlerVertexAiLlmConfig::from_env()?)
        } else {
            CrawlerLlmConfig::Disabled
        };

        info!(
            authoring_mode = authoring.as_str(),
            authoring_mode_source = source.as_str(),
            llm_enabled = authoring.uses_llm(),
            "Crawler authoring mode configured: {}",
            authoring_summary(authoring)
        );
        if let CrawlerLlmConfig::VertexAi(vertex) = &llm {
            info!(
                llm_provider = "vertex_ai",
                schema_model = %vertex.models.product_schema,
                url_classification_model = %vertex.models.url_classification,
                max_concurrent_requests = vertex.rate_limit.max_concurrent_requests,
                min_request_interval_ms = vertex.rate_limit.min_request_interval.as_millis(),
                "Crawler LLM governor configured"
            );
        }
        Ok(Self {
            authoring,
            llm,
            shutdown_grace_period: DEFAULT_SHUTDOWN_GRACE_PERIOD,
        })
    }
}

impl CrawlerVertexAiLlmConfig {
    fn from_env() -> Result<Self, CrawlerRuntimeConfigError> {
        Ok(Self {
            vertex_ai: CrawlerVertexAiConfig::from_env()?,
            models: CrawlerVertexAiModels::from_env(),
            rate_limit: CrawlerLlmRateLimitConfig::from_env(),
        })
    }
}

/// One sentence an operator can read in the startup log to know what the mode does.
fn authoring_summary(authoring: CrawlerAuthoringMode) -> &'static str {
    match authoring {
        CrawlerAuthoringMode::Auto => {
            "LLM generates; inferred URL patterns apply directly and high-confidence schemas are auto-approved"
        }
        CrawlerAuthoringMode::Review => {
            "LLM generates; every URL pattern and schema waits for human review (set CRAWLER_AUTHORING_MODE=auto to auto-approve)"
        }
        CrawlerAuthoringMode::Manual => {
            "no LLM calls; operators write URL patterns and schemas in the review console"
        }
    }
}

/// The trimmed value of `name`; unset, empty and whitespace-only values count as unset so
/// `NAME=` placeholders keep the default. A non-UTF-8 value is returned as `Err` for reporting.
fn env_value(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(raw.trim().to_owned())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(raw)) => Err(raw.to_string_lossy().into_owned()),
    }
}

/// Reads an enumerated setting and where it came from. A set but unrecognized value fails
/// startup instead of silently changing crawler policy.
fn enum_from_env<T: Default>(
    name: &'static str,
    parse: impl Fn(&str) -> Option<T>,
    accepted: &[&str],
) -> Result<(T, ConfigSource), CrawlerRuntimeConfigError> {
    let invalid = |value: String| CrawlerRuntimeConfigError::InvalidValue {
        name,
        value,
        accepted: accepted.join(", "),
    };
    match env_value(name).map_err(invalid)? {
        Some(raw) => parse(&raw)
            .map(|value| (value, ConfigSource::Environment))
            .ok_or_else(|| invalid(raw)),
        None => Ok((T::default(), ConfigSource::Default)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serial_test::serial;

    // Tests that touch the process environment are serialized, so no test reads the
    // environment while another one changes it.
    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    const RUNTIME_ENV: [&str; 6] = [
        AUTHORING_MODE_ENV,
        "CRAWLER_REVIEW_REQUIRED",
        "CRAWLER_REVIEW_URL_PATTERN_REQUIRED",
        "CRAWLER_SCHEMA_LLM_REVIEW_MODE",
        "VERTEX_AI_PROJECT_ID",
        "VERTEX_AI_LOCATION",
    ];

    /// Reads the runtime config from exactly `env`, leaving the process environment clean.
    fn runtime_config_from(
        env: &[(&str, &str)],
    ) -> Result<CrawlerRuntimeConfig, CrawlerRuntimeConfigError> {
        for name in RUNTIME_ENV {
            set_env(name, None);
        }
        for (name, value) in env {
            set_env(name, Some(value));
        }
        let config = CrawlerRuntimeConfig::from_env();
        for name in RUNTIME_ENV {
            set_env(name, None);
        }
        config
    }

    const VERTEX_ENV: [(&str, &str); 2] = [
        ("VERTEX_AI_PROJECT_ID", "test-project"),
        ("VERTEX_AI_LOCATION", "europe-west3"),
    ];

    fn vertex_env_with(
        extra: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        VERTEX_ENV.iter().chain(extra).copied().collect()
    }

    fn expect_vertex(config: &CrawlerRuntimeConfig) -> &CrawlerVertexAiLlmConfig {
        match &config.llm {
            CrawlerLlmConfig::VertexAi(vertex) => vertex,
            CrawlerLlmConfig::Disabled => panic!("expected the Vertex AI LLM configuration"),
        }
    }

    #[rstest]
    #[case::auto("auto", CrawlerAuthoringMode::Auto)]
    #[case::review("review", CrawlerAuthoringMode::Review)]
    #[case::mixed_case_with_whitespace(" Review ", CrawlerAuthoringMode::Review)]
    #[serial]
    fn should_read_llm_authoring_mode_with_vertex_ai_settings(
        #[case] value: &'static str,
        #[case] expected: CrawlerAuthoringMode,
    ) {
        let config = runtime_config_from(&vertex_env_with(&[(AUTHORING_MODE_ENV, value)]))
            .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(config.authoring, expected);
        assert_eq!(
            expect_vertex(&config).vertex_ai,
            CrawlerVertexAiConfig::new("test-project", "europe-west3")
        );
    }

    #[rstest]
    #[case::unset(None)]
    #[case::empty(Some(""))]
    #[case::whitespace(Some("  "))]
    #[serial]
    fn should_default_to_review_mode(#[case] value: Option<&'static str>) {
        let env = match value {
            Some(value) => vertex_env_with(&[(AUTHORING_MODE_ENV, value)]),
            None => VERTEX_ENV.to_vec(),
        };

        let config = runtime_config_from(&env)
            .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(config.authoring, CrawlerAuthoringMode::Review);
        assert!(matches!(config.llm, CrawlerLlmConfig::VertexAi(_)));
    }

    #[test]
    #[serial]
    fn should_disable_llm_without_vertex_ai_settings_in_manual_mode() {
        let config = runtime_config_from(&[(AUTHORING_MODE_ENV, "manual")])
            .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(config.authoring, CrawlerAuthoringMode::Manual);
        assert!(matches!(config.llm, CrawlerLlmConfig::Disabled));
    }

    #[rstest]
    #[case::unknown("jev_auto")]
    #[case::old_schema_mode("auto_approve_high_confidence")]
    #[case::old_report_only("report_only")]
    #[serial]
    fn should_reject_invalid_authoring_mode(#[case] value: &'static str) {
        let error = runtime_config_from(&vertex_env_with(&[(AUTHORING_MODE_ENV, value)]))
            .err()
            .unwrap_or_else(|| panic!("invalid authoring mode {value:?} must fail startup"));

        assert!(matches!(
            &error,
            CrawlerRuntimeConfigError::InvalidValue { name: "CRAWLER_AUTHORING_MODE", value: rejected, .. }
                if rejected == value
        ));
        assert_eq!(
            error.to_string(),
            format!(
                "invalid CRAWLER_AUTHORING_MODE value {value:?}; expected one of: auto, review, manual"
            )
        );
    }

    #[rstest]
    #[case::review_required("CRAWLER_REVIEW_REQUIRED", "true")]
    #[case::url_pattern_review_required("CRAWLER_REVIEW_URL_PATTERN_REQUIRED", "false")]
    #[case::schema_review_mode("CRAWLER_SCHEMA_LLM_REVIEW_MODE", "report_only")]
    #[serial]
    fn should_reject_removed_review_settings(
        #[case] name: &'static str,
        #[case] value: &'static str,
    ) {
        let error = runtime_config_from(&vertex_env_with(&[(name, value)]))
            .err()
            .unwrap_or_else(|| panic!("removed setting {name} must fail startup"));

        assert_eq!(
            error.to_string(),
            format!(
                "{name} was removed; set CRAWLER_AUTHORING_MODE (auto, review or manual) instead"
            )
        );
    }

    /// A value that is not valid Unicode on the current platform.
    fn non_unicode_value() -> std::ffi::OsString {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff])
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xD800])
        }
    }

    #[test]
    #[serial]
    fn should_reject_non_unicode_removed_review_setting() {
        for name in RUNTIME_ENV {
            set_env(name, None);
        }
        for (name, value) in VERTEX_ENV {
            set_env(name, Some(value));
        }
        unsafe { std::env::set_var("CRAWLER_REVIEW_REQUIRED", non_unicode_value()) };

        let config = CrawlerRuntimeConfig::from_env();
        for name in RUNTIME_ENV {
            set_env(name, None);
        }

        assert!(matches!(
            config,
            Err(CrawlerRuntimeConfigError::RemovedSetting {
                name: "CRAWLER_REVIEW_REQUIRED",
                ..
            })
        ));
    }

    #[test]
    #[serial]
    fn should_ignore_empty_removed_review_settings() {
        let config =
            runtime_config_from(&vertex_env_with(&[("CRAWLER_SCHEMA_LLM_REVIEW_MODE", "")]))
                .unwrap_or_else(|error| panic!("runtime config from env: {error}"));

        assert_eq!(config.authoring, CrawlerAuthoringMode::Review);
    }

    #[test]
    #[serial]
    fn should_reject_runtime_config_without_vertex_ai_project() {
        let config = runtime_config_from(&[("VERTEX_AI_LOCATION", "europe-west3")]);

        assert!(matches!(
            config,
            Err(CrawlerRuntimeConfigError::VertexAi(
                CrawlerVertexAiConfigError::MissingEnvironment {
                    name: "VERTEX_AI_PROJECT_ID"
                }
            ))
        ));
    }
}
