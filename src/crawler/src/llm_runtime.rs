use application::error::static_error;
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::{Instant, sleep};
use tracing::warn;

const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 1;
const DEFAULT_MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const MAX_GENERATION_ATTEMPTS: usize = 3;
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(250);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrawlerLlmRateLimitConfig {
    pub max_concurrent_requests: usize,
    pub min_request_interval: Duration,
}

impl Default for CrawlerLlmRateLimitConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            min_request_interval: DEFAULT_MIN_REQUEST_INTERVAL,
        }
    }
}

impl CrawlerLlmRateLimitConfig {
    pub fn from_env() -> Self {
        let default = Self::default();
        let max_concurrent_requests = std::env::var("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default.max_concurrent_requests);
        let min_request_interval = std::env::var("CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_millis)
            .unwrap_or(default.min_request_interval);

        Self {
            max_concurrent_requests,
            min_request_interval,
        }
    }
}

pub struct CrawlerLlmGovernor {
    config: CrawlerLlmRateLimitConfig,
    semaphore: Semaphore,
    last_request_started_at: Mutex<Option<Instant>>,
}

impl CrawlerLlmGovernor {
    pub fn new(config: CrawlerLlmRateLimitConfig) -> Self {
        Self {
            semaphore: Semaphore::new(config.max_concurrent_requests),
            config,
            last_request_started_at: Mutex::new(None),
        }
    }

    async fn acquire(&self) -> Result<CrawlerLlmPermit<'_>, LargeLanguageModelError> {
        let permit =
            self.semaphore
                .acquire()
                .await
                .map_err(|_| LargeLanguageModelError::Retryable {
                    source: static_error("crawler LLM governor closed"),
                })?;
        self.wait_for_start_slot().await;
        Ok(CrawlerLlmPermit { _permit: permit })
    }

    async fn wait_for_start_slot(&self) {
        let mut last_request_started_at = self.last_request_started_at.lock().await;
        if let Some(last_start) = *last_request_started_at {
            let elapsed = last_start.elapsed();
            if elapsed < self.config.min_request_interval {
                sleep(self.config.min_request_interval - elapsed).await;
            }
        }
        *last_request_started_at = Some(Instant::now());
    }
}

struct CrawlerLlmPermit<'a> {
    _permit: SemaphorePermit<'a>,
}

pub async fn generate_with_governor<Model, Output>(
    model: &Model,
    governor: Option<&Arc<CrawlerLlmGovernor>>,
    request: StructuredGenerationRequest,
) -> Result<Output, LargeLanguageModelError>
where
    Model: LargeLanguageModel,
    Output: DeserializeOwned + Send,
{
    for attempt in 1..=MAX_GENERATION_ATTEMPTS {
        let permit = match governor {
            Some(governor) => Some(governor.acquire().await?),
            None => None,
        };
        let result = model.generate(request.clone()).await;
        drop(permit);

        match result {
            Ok(output) => return Ok(output),
            Err(error) if should_retry(&error) && attempt < MAX_GENERATION_ATTEMPTS => {
                let delay = retry_delay(attempt);
                warn!(
                    operation = ?request.operation,
                    attempt,
                    max_attempts = MAX_GENERATION_ATTEMPTS,
                    delay_ms = delay.as_millis(),
                    error_kind = error_kind(&error),
                    "Retrying crawler LLM generation"
                );
                sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(LargeLanguageModelError::Retryable {
        source: static_error("crawler LLM retry loop exhausted unexpectedly"),
    })
}

fn should_retry(error: &LargeLanguageModelError) -> bool {
    matches!(
        error,
        LargeLanguageModelError::Timeout { .. } | LargeLanguageModelError::Retryable { .. }
    )
}

fn retry_delay(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    RETRY_BACKOFF_BASE
        .checked_mul(2_u32.saturating_pow(exponent))
        .unwrap_or(RETRY_BACKOFF_MAX)
        .min(RETRY_BACKOFF_MAX)
}

fn error_kind(error: &LargeLanguageModelError) -> &'static str {
    match error {
        LargeLanguageModelError::Authentication { .. } => "authentication",
        LargeLanguageModelError::Timeout { .. } => "timeout",
        LargeLanguageModelError::Retryable { .. } => "retryable",
        LargeLanguageModelError::Permanent { .. } => "permanent",
        LargeLanguageModelError::InvalidResponse { .. } => "invalid_response",
    }
}
