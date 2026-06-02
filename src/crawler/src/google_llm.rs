use llm::backends::google::{GooglePlatform, GoogleServiceTier};
use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::{ChatMessage, ChatProvider, ChatResponse};
use llm::error::LLMError;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::{Instant, sleep};
use tracing::warn;

const DEFAULT_GEMINI_MAX_CONCURRENT_REQUESTS: usize = 1;
const DEFAULT_GEMINI_MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const GEMINI_CHAT_MAX_ATTEMPTS: usize = 3;
const GEMINI_RATE_LIMIT_DELAY: Duration = Duration::from_secs(30);
const GEMINI_SERVICE_UNAVAILABLE_DELAY: Duration = Duration::from_secs(10 * 60);
const GEMINI_TRANSIENT_ERROR_DELAY: Duration = Duration::from_secs(1);
#[cfg(test)]
const GEMINI_TEST_RETRY_DELAY: Duration = Duration::from_millis(1);

pub fn google_llm_builder(api_key: &str, model: &str, gemini_flex: bool) -> LLMBuilder {
    let builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .google_platform(GooglePlatform::GeminiEnterpriseAgent {
            project_id: "aura-historial".to_owned(),
            region: Some("europe-west3".to_owned()),
        })
        .api_key(api_key)
        .model(model);

    if gemini_flex {
        builder.google_service_tier(GoogleServiceTier::Flex)
    } else {
        builder
    }
}

pub fn gemini_flex_enabled() -> bool {
    std::env::var("GEMINI_FLEX")
        .ok()
        .is_some_and(|raw| parse_gemini_flex(&raw))
}

fn parse_gemini_flex(raw: &str) -> bool {
    let raw = raw.trim();
    raw == "1" || raw.eq_ignore_ascii_case("true")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeminiRateLimitConfig {
    pub max_concurrent_requests: usize,
    pub min_request_interval: Duration,
}

impl Default for GeminiRateLimitConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: DEFAULT_GEMINI_MAX_CONCURRENT_REQUESTS,
            min_request_interval: DEFAULT_GEMINI_MIN_REQUEST_INTERVAL,
        }
    }
}

impl GeminiRateLimitConfig {
    pub fn from_env() -> Self {
        Self {
            max_concurrent_requests: parse_positive_usize_env(
                "GEMINI_MAX_CONCURRENT_REQUESTS",
                DEFAULT_GEMINI_MAX_CONCURRENT_REQUESTS,
            ),
            min_request_interval: Duration::from_millis(parse_positive_u64_env(
                "GEMINI_MIN_REQUEST_INTERVAL_MS",
                DEFAULT_GEMINI_MIN_REQUEST_INTERVAL.as_millis() as u64,
            )),
        }
    }
}

fn parse_positive_usize_env(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_positive_u64_env(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

#[derive(Debug)]
pub struct GeminiRateLimiter {
    config: GeminiRateLimitConfig,
    semaphore: Semaphore,
    last_request_started_at: Mutex<Option<Instant>>,
}

#[derive(Debug)]
pub(crate) struct GeminiRateLimitPermit<'a> {
    #[expect(dead_code, reason = "RAII guard holds the semaphore permit until drop")]
    permit: SemaphorePermit<'a>,
}

impl GeminiRateLimiter {
    pub fn new(config: GeminiRateLimitConfig) -> Self {
        Self {
            config,
            semaphore: Semaphore::new(config.max_concurrent_requests),
            last_request_started_at: Mutex::new(None),
        }
    }

    async fn wait_for_start_slot(&self) {
        let mut last_request_started_at = self.last_request_started_at.lock().await;

        if let Some(last_started_at) = *last_request_started_at {
            let elapsed = last_started_at.elapsed();
            if elapsed < self.config.min_request_interval {
                sleep(self.config.min_request_interval - elapsed).await;
            }
        }

        *last_request_started_at = Some(Instant::now());
    }

    pub(crate) async fn acquire(&self) -> Result<GeminiRateLimitPermit<'_>, LLMError> {
        let permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| LLMError::ProviderError("Gemini rate limiter closed".to_string()))?;
        self.wait_for_start_slot().await;

        Ok(GeminiRateLimitPermit { permit })
    }
}

pub(crate) async fn run_with_gemini_rate_limiter(
    llm: &dyn ChatProvider,
    rate_limiter: Option<&GeminiRateLimiter>,
    messages: &[ChatMessage],
) -> Result<Box<dyn ChatResponse>, LLMError> {
    for attempt in 1..=GEMINI_CHAT_MAX_ATTEMPTS {
        let permit = match rate_limiter {
            Some(limiter) => Some(limiter.acquire().await?),
            None => None,
        };
        let result = llm.chat(messages).await;
        drop(permit);

        match result {
            Ok(response) => return Ok(response),
            Err(error) => {
                let Some(status_code) = retryable_gemini_status_code(&error) else {
                    return Err(error);
                };
                if attempt == GEMINI_CHAT_MAX_ATTEMPTS {
                    return Err(error);
                }

                let delay = gemini_retry_sleep_delay(status_code, attempt - 1);
                warn!(
                    status_code,
                    attempt,
                    max_attempts = GEMINI_CHAT_MAX_ATTEMPTS,
                    delay_ms = delay.as_millis(),
                    error = %error,
                    "Retrying Gemini chat request after retryable provider error"
                );
                sleep(delay).await;
            }
        }
    }

    unreachable!("Gemini chat retry loop always returns from an attempt")
}

fn retryable_gemini_status_code(error: &LLMError) -> Option<u16> {
    let status_code = llm_error_status_code(error)?;
    is_retryable_gemini_status_code(status_code).then_some(status_code)
}

fn is_retryable_gemini_status_code(status_code: u16) -> bool {
    matches!(status_code, 408 | 429 | 500 | 502 | 503 | 504)
}

fn gemini_retry_delay(status_code: u16, attempt_index: usize) -> Duration {
    let base_delay = match status_code {
        429 => GEMINI_RATE_LIMIT_DELAY,
        503 => GEMINI_SERVICE_UNAVAILABLE_DELAY,
        408 | 500 | 502 | 504 => GEMINI_TRANSIENT_ERROR_DELAY,
        _ => return Duration::ZERO,
    };

    base_delay.saturating_mul(1_u32 << attempt_index.min(4))
}

#[cfg(not(test))]
fn gemini_retry_sleep_delay(status_code: u16, attempt_index: usize) -> Duration {
    gemini_retry_delay(status_code, attempt_index)
}

#[cfg(test)]
fn gemini_retry_sleep_delay(_status_code: u16, _attempt_index: usize) -> Duration {
    GEMINI_TEST_RETRY_DELAY
}

fn llm_error_status_code(error: &LLMError) -> Option<u16> {
    match error {
        LLMError::HttpError(message)
        | LLMError::ProviderError(message)
        | LLMError::Generic(message) => status_code_in_message(message),
        LLMError::RetryExceeded { last_error, .. } => status_code_in_message(last_error),
        LLMError::AuthError(_)
        | LLMError::InvalidRequest(_)
        | LLMError::ResponseFormatError { .. }
        | LLMError::JsonError(_)
        | LLMError::ToolConfigError(_) => None,
    }
}

fn status_code_in_message(message: &str) -> Option<u16> {
    message
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|token| token.parse::<u16>().ok())
        .find(|status_code| (400..=599).contains(status_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::chat::Tool;
    use std::collections::VecDeque;
    use std::fmt::{Display, Formatter};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
    use tokio::sync::Barrier;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().expect("env lock should not be poisoned")
    }

    #[test]
    fn should_enable_flex_for_truthy_env() {
        assert!(parse_gemini_flex("true"));
        assert!(parse_gemini_flex("1"));
    }

    #[test]
    fn should_disable_flex_for_falsey_or_missing_env() {
        assert!(!parse_gemini_flex("false"));
        assert!(!parse_gemini_flex("0"));
    }

    #[test]
    fn should_ignore_invalid_env_values() {
        assert!(!parse_gemini_flex("sometimes"));
        assert!(!parse_gemini_flex(""));
    }

    #[test]
    fn should_enable_flex_case_insensitively_for_true() {
        assert!(parse_gemini_flex("TRUE"));
    }

    #[test]
    fn should_ignore_surrounding_whitespace() {
        assert!(parse_gemini_flex(" true "));
    }

    #[test]
    fn should_use_default_rate_limit_config_when_env_missing_or_invalid() {
        let env_lock = lock_env();
        unsafe {
            std::env::remove_var("GEMINI_MAX_CONCURRENT_REQUESTS");
            std::env::set_var("GEMINI_MIN_REQUEST_INTERVAL_MS", "0");
        }

        let config = GeminiRateLimitConfig::from_env();

        assert_eq!(
            config.max_concurrent_requests,
            DEFAULT_GEMINI_MAX_CONCURRENT_REQUESTS
        );
        assert_eq!(
            config.min_request_interval,
            DEFAULT_GEMINI_MIN_REQUEST_INTERVAL
        );
        drop(env_lock);
    }

    #[test]
    fn should_parse_rate_limit_config_from_env() {
        let env_lock = lock_env();
        unsafe {
            std::env::set_var("GEMINI_MAX_CONCURRENT_REQUESTS", "2");
            std::env::set_var("GEMINI_MIN_REQUEST_INTERVAL_MS", "50");
        }

        let config = GeminiRateLimitConfig::from_env();

        assert_eq!(config.max_concurrent_requests, 2);
        assert_eq!(config.min_request_interval, Duration::from_millis(50));
        drop(env_lock);
    }

    #[test]
    fn should_classify_retryable_gemini_status_codes() {
        for status_code in [408, 429, 500, 502, 503, 504] {
            let error =
                LLMError::HttpError(format!("HTTP status server error ({status_code}) for url"));

            assert_eq!(retryable_gemini_status_code(&error), Some(status_code));
        }
    }

    #[test]
    fn should_not_retry_non_transient_gemini_status_codes() {
        for status_code in [400, 401, 403, 404, 422] {
            let error =
                LLMError::HttpError(format!("HTTP status client error ({status_code}) for url"));

            assert_eq!(retryable_gemini_status_code(&error), None);
        }
    }

    #[test]
    fn should_use_configured_rate_limit_retry_backoff() {
        assert_eq!(gemini_retry_delay(429, 0), Duration::from_secs(30));
        assert_eq!(gemini_retry_delay(429, 1), Duration::from_secs(60));
    }

    #[test]
    fn should_use_configured_service_unavailable_retry_backoff() {
        assert_eq!(gemini_retry_delay(503, 0), Duration::from_secs(10 * 60));
        assert_eq!(gemini_retry_delay(503, 1), Duration::from_secs(20 * 60));
    }

    #[test]
    fn should_keep_short_transient_error_retry_backoff() {
        for status_code in [408, 500, 502, 504] {
            assert_eq!(
                gemini_retry_delay(status_code, 0),
                Duration::from_secs(1),
                "unexpected first retry delay for status {status_code}"
            );
            assert_eq!(
                gemini_retry_delay(status_code, 1),
                Duration::from_secs(2),
                "unexpected second retry delay for status {status_code}"
            );
        }
    }

    #[tokio::test]
    async fn should_retry_chat_after_retryable_status_error() {
        let provider = SequenceChatProvider::new([
            Err(LLMError::HttpError(
                "HTTP status client error (429 Too Many Requests) for url".to_string(),
            )),
            Ok("ok"),
        ]);
        let messages = [ChatMessage::user().content("hello").build()];

        let response = run_with_gemini_rate_limiter(&provider, None, &messages)
            .await
            .expect("retryable request should eventually succeed");

        assert_eq!(response.text().as_deref(), Some("ok"));
        assert_eq!(provider.attempts(), 2);
    }

    #[tokio::test]
    async fn should_not_retry_chat_after_non_retryable_status_error() {
        let provider = SequenceChatProvider::new([
            Err(LLMError::HttpError(
                "HTTP status client error (400 Bad Request) for url".to_string(),
            )),
            Ok("should not be used"),
        ]);
        let messages = [ChatMessage::user().content("hello").build()];

        let error = run_with_gemini_rate_limiter(&provider, None, &messages)
            .await
            .expect_err("non-retryable request should fail immediately");

        assert!(matches!(error, LLMError::HttpError(_)));
        assert_eq!(provider.attempts(), 1);
    }

    #[tokio::test]
    async fn should_stop_retrying_after_max_attempts() {
        let provider = SequenceChatProvider::new([
            Err(LLMError::HttpError(
                "HTTP status server error (503 Service Unavailable) for url".to_string(),
            )),
            Err(LLMError::HttpError(
                "HTTP status server error (503 Service Unavailable) for url".to_string(),
            )),
            Err(LLMError::HttpError(
                "HTTP status server error (503 Service Unavailable) for url".to_string(),
            )),
        ]);
        let messages = [ChatMessage::user().content("hello").build()];

        let error = run_with_gemini_rate_limiter(&provider, None, &messages)
            .await
            .expect_err("exhausted retries should return the last error");

        assert!(matches!(error, LLMError::HttpError(_)));
        assert_eq!(provider.attempts(), GEMINI_CHAT_MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn should_limit_concurrent_requests() {
        ACTIVE_REQUESTS.store(0, Ordering::SeqCst);
        MAX_ACTIVE_REQUESTS.store(0, Ordering::SeqCst);

        let limiter = Arc::new(GeminiRateLimiter::new(GeminiRateLimitConfig {
            max_concurrent_requests: 1,
            min_request_interval: Duration::from_millis(1),
        }));

        let barrier = Arc::new(Barrier::new(3));
        let first = {
            let limiter = Arc::clone(&limiter);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                let permit = limiter.acquire().await?;
                track_concurrent_request().await;
                drop(permit);
                Ok::<_, LLMError>(())
            })
        };
        let second = {
            let limiter = Arc::clone(&limiter);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                let permit = limiter.acquire().await?;
                track_concurrent_request().await;
                drop(permit);
                Ok::<_, LLMError>(())
            })
        };

        barrier.wait().await;

        first.await.expect("first task should join").unwrap();
        second.await.expect("second task should join").unwrap();

        assert_eq!(ConcurrentTrackingLlm::max_seen(), 1);
    }

    #[tokio::test]
    async fn should_pace_request_starts() {
        let limiter = Arc::new(GeminiRateLimiter::new(GeminiRateLimitConfig {
            max_concurrent_requests: 2,
            min_request_interval: Duration::from_millis(25),
        }));
        let first_started_at = Instant::now();
        let first_permit = limiter.acquire().await.expect("first request should pass");
        let second_permit = limiter.acquire().await.expect("second request should pass");

        assert!(first_started_at.elapsed() >= Duration::from_millis(25));
        drop(second_permit);
        drop(first_permit);
    }

    static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);
    static MAX_ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

    struct ConcurrentTrackingLlm;

    impl ConcurrentTrackingLlm {
        fn max_seen() -> usize {
            MAX_ACTIVE_REQUESTS.load(Ordering::SeqCst)
        }
    }

    async fn track_concurrent_request() {
        let active = ACTIVE_REQUESTS.fetch_add(1, Ordering::SeqCst) + 1;
        MAX_ACTIVE_REQUESTS.fetch_max(active, Ordering::SeqCst);
        sleep(Duration::from_millis(20)).await;
        ACTIVE_REQUESTS.fetch_sub(1, Ordering::SeqCst);
    }

    struct SequenceChatProvider {
        responses: StdMutex<VecDeque<Result<&'static str, LLMError>>>,
        attempts: AtomicUsize,
    }

    impl SequenceChatProvider {
        fn new<const N: usize>(responses: [Result<&'static str, LLMError>; N]) -> Self {
            Self {
                responses: StdMutex::new(VecDeque::from(responses)),
                attempts: AtomicUsize::new(0),
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ChatProvider for SequenceChatProvider {
        async fn chat_with_tools(
            &self,
            messages: &[ChatMessage],
            tools: Option<&[Tool]>,
        ) -> Result<Box<dyn ChatResponse>, LLMError> {
            assert!(!messages.is_empty());
            assert!(tools.is_none());
            self.attempts.fetch_add(1, Ordering::SeqCst);

            let response = self
                .responses
                .lock()
                .expect("responses lock should not be poisoned")
                .pop_front()
                .expect("test should configure enough responses");

            response.map(|text| Box::new(TextChatResponse(text)) as Box<dyn ChatResponse>)
        }
    }

    #[derive(Debug)]
    struct TextChatResponse(&'static str);

    impl Display for TextChatResponse {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl ChatResponse for TextChatResponse {
        fn text(&self) -> Option<String> {
            Some(self.0.to_string())
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }
}
