use llm::backends::google::GoogleServiceTier;
use llm::builder::{LLMBackend, LLMBuilder};
use llm::error::LLMError;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::{Instant, sleep};

const DEFAULT_GEMINI_MAX_CONCURRENT_REQUESTS: usize = 1;
const DEFAULT_GEMINI_MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);

pub fn google_llm_builder(api_key: &str, model: &str, gemini_flex: bool) -> LLMBuilder {
    let builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
