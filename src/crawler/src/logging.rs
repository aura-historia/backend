use std::time::Duration;

use async_trait::async_trait;
use common::logging::{
    LlmInvocationMetrics, LlmOperation, LlmProvider, log_llm_invocation_with_context,
};

pub const CRAWLER_SERVICE_NAME: &str = "crawler";
pub const COMPONENT_CRON: &str = "cron";
pub const COMPONENT_LLM: &str = "llm";
pub const COMPONENT_SCRAPER: &str = "scraper";
pub const COMPONENT_SHOP_SYNC: &str = "shop_sync";
pub const COMPONENT_SPIDER: &str = "spider";
pub const COMPONENT_STARTUP: &str = "startup";

const DEFAULT_LOG_STREAM_NAME: &str = "unknown-host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudWatchLoggingConfig {
    pub log_group_name: String,
    pub log_stream_name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CloudWatchLoggingConfigError {
    #[error("CRAWLER_CLOUDWATCH_LOG_GROUP must not be blank when provided")]
    EmptyLogGroup,

    #[error("CRAWLER_CLOUDWATCH_LOG_STREAM must not be blank when provided")]
    EmptyLogStream,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CloudWatchBootstrapError {
    #[error("CloudWatch destination already exists")]
    AlreadyExists,

    #[error("{0}")]
    Other(String),
}

pub fn cloudwatch_logging_config()
-> Result<Option<CloudWatchLoggingConfig>, CloudWatchLoggingConfigError> {
    let Some(log_group_name) = std::env::var("CRAWLER_CLOUDWATCH_LOG_GROUP").ok() else {
        return Ok(None);
    };

    let log_group_name = log_group_name.trim();
    if log_group_name.is_empty() {
        return Err(CloudWatchLoggingConfigError::EmptyLogGroup);
    }

    let log_stream_name = match std::env::var("CRAWLER_CLOUDWATCH_LOG_STREAM") {
        Ok(stream) => {
            let stream = stream.trim();
            if stream.is_empty() {
                return Err(CloudWatchLoggingConfigError::EmptyLogStream);
            }
            stream.to_string()
        }
        Err(_) => default_log_stream_name(),
    };

    Ok(Some(CloudWatchLoggingConfig {
        log_group_name: log_group_name.to_string(),
        log_stream_name,
    }))
}

pub fn default_log_stream_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_STREAM_NAME.to_string())
}

pub fn current_gemini_model() -> String {
    std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "unknown".to_string())
}

pub fn llm_metrics(
    usage: Option<llm::chat::Usage>,
    batch_size: Option<usize>,
) -> LlmInvocationMetrics {
    let Some(usage) = usage else {
        return LlmInvocationMetrics {
            batch_size,
            ..Default::default()
        };
    };

    LlmInvocationMetrics {
        batch_size,
        prompt_tokens: Some(usage.prompt_tokens),
        completion_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cached_prompt_tokens: usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens),
        ..Default::default()
    }
}

pub fn log_crawler_llm_invocation(
    operation: LlmOperation,
    model: &str,
    latency: Duration,
    metrics: LlmInvocationMetrics,
) {
    log_llm_invocation_with_context(
        operation.as_str(),
        LlmProvider::Google.as_str(),
        model,
        latency,
        metrics,
        Some(CRAWLER_SERVICE_NAME),
        Some(COMPONENT_LLM),
    );
}

#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait CloudWatchBootstrapClient: Send + Sync {
    async fn create_log_group(&self, log_group_name: &str) -> Result<(), CloudWatchBootstrapError>;

    async fn create_log_stream(
        &self,
        log_group_name: &str,
        log_stream_name: &str,
    ) -> Result<(), CloudWatchBootstrapError>;
}

pub async fn ensure_cloudwatch_log_destination(
    client: &dyn CloudWatchBootstrapClient,
    config: &CloudWatchLoggingConfig,
) -> Result<(), CloudWatchBootstrapError> {
    match client.create_log_group(&config.log_group_name).await {
        Ok(()) | Err(CloudWatchBootstrapError::AlreadyExists) => {}
        Err(error) => return Err(error),
    }

    match client
        .create_log_stream(&config.log_group_name, &config.log_stream_name)
        .await
    {
        Ok(()) | Err(CloudWatchBootstrapError::AlreadyExists) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    #[serial]
    fn should_disable_cloudwatch_logging_when_log_group_env_missing() {
        set_env("CRAWLER_CLOUDWATCH_LOG_GROUP", None);
        set_env("CRAWLER_CLOUDWATCH_LOG_STREAM", None);

        let result = cloudwatch_logging_config().unwrap();

        assert_eq!(result, None);
    }

    #[test]
    #[serial]
    fn should_use_hostname_when_log_stream_env_missing_for_cloudwatch_logging() {
        set_env("CRAWLER_CLOUDWATCH_LOG_GROUP", Some("crawler-prod"));
        set_env("CRAWLER_CLOUDWATCH_LOG_STREAM", None);
        set_env("HOSTNAME", Some("crawler-host"));
        set_env("COMPUTERNAME", None);

        let result = cloudwatch_logging_config().unwrap();

        assert_eq!(
            result,
            Some(CloudWatchLoggingConfig {
                log_group_name: "crawler-prod".to_string(),
                log_stream_name: "crawler-host".to_string(),
            })
        );
    }

    #[test]
    #[serial]
    fn should_error_when_blank_log_group_env_provided_for_cloudwatch_logging() {
        set_env("CRAWLER_CLOUDWATCH_LOG_GROUP", Some("   "));
        set_env("CRAWLER_CLOUDWATCH_LOG_STREAM", None);

        let result = cloudwatch_logging_config();

        assert_eq!(result, Err(CloudWatchLoggingConfigError::EmptyLogGroup));
    }

    #[test]
    #[serial]
    fn should_error_when_blank_log_stream_env_provided_for_cloudwatch_logging() {
        set_env("CRAWLER_CLOUDWATCH_LOG_GROUP", Some("crawler-prod"));
        set_env("CRAWLER_CLOUDWATCH_LOG_STREAM", Some("   "));

        let result = cloudwatch_logging_config();

        assert_eq!(result, Err(CloudWatchLoggingConfigError::EmptyLogStream));
    }

    #[tokio::test]
    async fn should_create_log_group_and_stream_for_cloudwatch_destination() {
        let mut client = MockCloudWatchBootstrapClient::new();
        client
            .expect_create_log_group()
            .with(mockall::predicate::eq("crawler-prod"))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        client
            .expect_create_log_stream()
            .with(
                mockall::predicate::eq("crawler-prod"),
                mockall::predicate::eq("crawler-host"),
            )
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let result = ensure_cloudwatch_log_destination(
            &client,
            &CloudWatchLoggingConfig {
                log_group_name: "crawler-prod".to_string(),
                log_stream_name: "crawler-host".to_string(),
            },
        )
        .await;

        assert_eq!(result, Ok(()));
    }

    #[tokio::test]
    async fn should_ignore_already_exists_errors_for_cloudwatch_destination() {
        let mut client = MockCloudWatchBootstrapClient::new();
        client
            .expect_create_log_group()
            .times(1)
            .returning(|_| Box::pin(async { Err(CloudWatchBootstrapError::AlreadyExists) }));
        client
            .expect_create_log_stream()
            .times(1)
            .returning(|_, _| Box::pin(async { Err(CloudWatchBootstrapError::AlreadyExists) }));

        let result = ensure_cloudwatch_log_destination(
            &client,
            &CloudWatchLoggingConfig {
                log_group_name: "crawler-prod".to_string(),
                log_stream_name: "crawler-host".to_string(),
            },
        )
        .await;

        assert_eq!(result, Ok(()));
    }
}
