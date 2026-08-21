use async_trait::async_trait;

pub const CRAWLER_SERVICE_NAME: &str = "crawler";
pub const HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE: &str = "html5ever::tree_builder=error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlerComponent {
    Startup,
    Cron,
    ShopSync,
    Spider,
    Scraper,
    Llm,
}

impl CrawlerComponent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Cron => "cron",
            Self::ShopSync => "shop_sync",
            Self::Spider => "spider",
            Self::Scraper => "scraper",
            Self::Llm => "llm",
        }
    }
}

impl std::fmt::Display for CrawlerComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const COMPONENT_CRON: &str = CrawlerComponent::Cron.as_str();
pub const COMPONENT_LLM: &str = CrawlerComponent::Llm.as_str();
pub const COMPONENT_SCRAPER: &str = CrawlerComponent::Scraper.as_str();
pub const COMPONENT_SHOP_SYNC: &str = CrawlerComponent::ShopSync.as_str();
pub const COMPONENT_SPIDER: &str = CrawlerComponent::Spider.as_str();
pub const COMPONENT_STARTUP: &str = CrawlerComponent::Startup.as_str();

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
    service_tier: Option<large_language_model::GeminiServiceTier>,
) -> large_language_model::LlmInvocationMetrics {
    let Some(usage) = usage else {
        return large_language_model::LlmInvocationMetrics {
            service_tier,
            batch_size,
            ..Default::default()
        };
    };

    large_language_model::LlmInvocationMetrics {
        service_tier,
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

    #[test]
    fn should_format_llm_crawler_component_name() {
        assert_eq!(CrawlerComponent::Llm.as_str(), "llm");
        assert_eq!(CrawlerComponent::Llm.to_string(), "llm");
    }
}
