use async_trait::async_trait;

pub const CRAWLER_SERVICE_NAME: &str = "crawler";
pub const HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE: &str = "html5ever::tree_builder=error";
/// The spider library logs its own crawl hard-timeout warning outside any span and with the
/// source path of the machine that built it. The crawl's `crawler.spider.crawl_completed` event
/// carries the truncation with the source, so only the library's errors stay.
pub const SPIDER_LIBRARY_LOG_DIRECTIVE: &str = "spider=error";
/// The platform certificate verifier logs every rejected certificate at ERROR, with no source and
/// with the certificate's full name list, once per handshake. A shop's broken certificate is no
/// crawler fault: the request fails, and the crawler's own events report it with the source, a
/// crawl as a `TlsError` on `crawler.spider.crawl_failed` at WARN with the domain. The verifier
/// logs nothing else per request; a trust store the crawler cannot load is reported by
/// `crate::network::tls` itself.
pub const TLS_VERIFIER_LOG_DIRECTIVE: &str = "rustls_platform_verifier=off";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlerComponent {
    Startup,
    Cron,
    ListingSourceSync,
    Spider,
    Scraper,
    Llm,
}

impl CrawlerComponent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Cron => "cron",
            Self::ListingSourceSync => "listing_source_sync",
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
pub const COMPONENT_LISTING_SOURCE_SYNC: &str = CrawlerComponent::ListingSourceSync.as_str();
pub const COMPONENT_SPIDER: &str = CrawlerComponent::Spider.as_str();
pub const COMPONENT_STARTUP: &str = CrawlerComponent::Startup.as_str();

/// The source a crawler outcome event is about, logged as the same three top-level fields on
/// every named event so one query follows a shop through spider, scraper and LLM logs. The span
/// list also carries source ids, but under keys that differ by span and not at all for some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLogContext {
    pub listing_source_id: listing_source_core::ListingSourceId,
    pub listing_source_name: String,
    /// The host the ListingSource is crawled on.
    pub domain: String,
}

tokio::task_local! {
    static CURRENT_SOURCE: SourceLogContext;
}

impl SourceLogContext {
    /// Runs `work` for this source: the outcome events of work that has no source parameter,
    /// such as an LLM invocation, log it through [`SourceLogContext::current`].
    pub async fn scope<F: std::future::Future>(self, work: F) -> F::Output {
        CURRENT_SOURCE.scope(self, work).await
    }

    /// The source the running work belongs to, when a caller scoped one.
    pub fn current() -> Option<Self> {
        CURRENT_SOURCE.try_with(Clone::clone).ok()
    }

    /// The source the running work belongs to when a caller scoped `listing_source_id`; else only
    /// the id, which is all an unscoped caller, such as a test or the dev binary, knows.
    pub fn of_run(listing_source_id: listing_source_core::ListingSourceId) -> Self {
        Self::current()
            .filter(|current| current.listing_source_id == listing_source_id)
            .unwrap_or_else(|| Self::new(listing_source_id, "", ""))
    }

    pub fn new(
        listing_source_id: listing_source_core::ListingSourceId,
        listing_source_name: impl Into<String>,
        domain: impl Into<String>,
    ) -> Self {
        Self {
            listing_source_id,
            listing_source_name: listing_source_name.into(),
            domain: domain.into(),
        }
    }
}

tokio::task_local! {
    static LLM_CALLS: std::cell::Cell<u32>;
}

/// Counts the provider calls of the work it scopes, so a caller that logs the work's outcome
/// reports how many calls it spent without the LLM runtime depending on that caller.
pub async fn count_llm_calls_in<F: std::future::Future>(work: F) -> F::Output {
    LLM_CALLS.scope(std::cell::Cell::new(0), work).await
}

/// Counts one provider call for the work scoped by [`count_llm_calls_in`]; outside it, nothing.
pub fn count_llm_call() {
    let _ = LLM_CALLS.try_with(|calls| calls.set(calls.get().saturating_add(1)));
}

/// The provider calls counted so far in the scoped work; `0` outside it.
pub fn llm_calls_counted() -> u32 {
    LLM_CALLS.try_with(std::cell::Cell::get).unwrap_or(0)
}

/// The source fields of the running work, for a record logged where only the ambient source is
/// at hand: the three top-level fields every outcome event carries, absent for unscoped work.
/// Use [`warn_in_source!`] and [`error_in_source!`] rather than building one by hand.
pub struct CurrentSourceFields(Option<SourceLogContext>);

impl SourceLogContext {
    pub fn current_fields() -> CurrentSourceFields {
        CurrentSourceFields(Self::current())
    }
}

impl CurrentSourceFields {
    pub fn listing_source_id(&self) -> Option<String> {
        self.0
            .as_ref()
            .map(|source| source.listing_source_id.to_string())
    }

    pub fn listing_source_name(&self) -> Option<&str> {
        self.0
            .as_ref()
            .map(|source| source.listing_source_name.as_str())
    }

    pub fn domain(&self) -> Option<&str> {
        self.0.as_ref().map(|source| source.domain.as_str())
    }
}

/// `tracing::warn!` that adds the ambient `listing_source_id`, `listing_source_name` and `domain`.
/// The JSON records carry the current span only, so a record of nested work names its source
/// itself.
macro_rules! warn_in_source {
    ($($rest:tt)*) => {{
        let source = $crate::logging::SourceLogContext::current_fields();
        tracing::warn!(
            listing_source_id = source.listing_source_id(),
            listing_source_name = source.listing_source_name(),
            domain = source.domain(),
            $($rest)*
        )
    }};
}

/// As [`warn_in_source!`] for `tracing::error!`.
macro_rules! error_in_source {
    ($($rest:tt)*) => {{
        let source = $crate::logging::SourceLogContext::current_fields();
        tracing::error!(
            listing_source_id = source.listing_source_id(),
            listing_source_name = source.listing_source_name(),
            domain = source.domain(),
            $($rest)*
        )
    }};
}

pub(crate) use {error_in_source, warn_in_source};

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
