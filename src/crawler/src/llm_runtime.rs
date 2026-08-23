use application::error::static_error;
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, LargeLanguageModelRetryAdvice,
    LargeLanguageModelRetryKind, StructuredGenerationRequest,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::{Instant, sleep};
use tracing::warn;

const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 1;
const DEFAULT_MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrawlerLlmRetryPolicy {
    pub max_attempts: usize,
    pub rate_limited_base: Duration,
    pub service_unavailable_base: Duration,
    pub transient_base: Duration,
    pub rate_limited_max: Duration,
    pub service_unavailable_max: Duration,
    pub transient_max: Duration,
    pub jitter_percent: u8,
}

impl Default for CrawlerLlmRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            rate_limited_base: Duration::from_secs(30),
            service_unavailable_base: Duration::from_secs(600),
            transient_base: Duration::from_secs(1),
            rate_limited_max: Duration::from_secs(300),
            service_unavailable_max: Duration::from_secs(3600),
            transient_max: Duration::from_secs(30),
            jitter_percent: 10,
        }
    }
}

pub struct CrawlerLlmGovernor {
    config: CrawlerLlmRateLimitConfig,
    semaphore: Semaphore,
    next_request_start_at: Mutex<Option<Instant>>,
}

impl CrawlerLlmGovernor {
    pub fn new(config: CrawlerLlmRateLimitConfig) -> Self {
        Self {
            semaphore: Semaphore::new(config.max_concurrent_requests),
            config,
            next_request_start_at: Mutex::new(None),
        }
    }

    async fn acquire(&self) -> Result<CrawlerLlmPermit<'_>, LargeLanguageModelError> {
        let permit =
            self.semaphore
                .acquire()
                .await
                .map_err(|_| LargeLanguageModelError::Retryable {
                    advice: LargeLanguageModelRetryAdvice {
                        kind: LargeLanguageModelRetryKind::Transient,
                        retry_after: None,
                    },
                    source: static_error("crawler LLM governor closed"),
                })?;

        let start_at = {
            let mut next = self.next_request_start_at.lock().await;

            let now = Instant::now();
            let reserved = match *next {
                Some(previous) => previous.max(now),
                None => now,
            };

            *next = Some(reserved + self.config.min_request_interval);
            reserved
        };

        tokio::time::sleep_until(start_at).await;

        Ok(CrawlerLlmPermit { _permit: permit })
    }
}

struct CrawlerLlmPermit<'a> {
    _permit: SemaphorePermit<'a>,
}

#[derive(Debug)]
pub enum ValidatedGenerationError<E> {
    Model(LargeLanguageModelError),
    Validation(E),
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
    generate_with_governor_and_policy(model, governor, request, CrawlerLlmRetryPolicy::default())
        .await
}

pub async fn generate_with_governor_and_policy<Model, Output>(
    model: &Model,
    governor: Option<&Arc<CrawlerLlmGovernor>>,
    request: StructuredGenerationRequest,
    policy: CrawlerLlmRetryPolicy,
) -> Result<Output, LargeLanguageModelError>
where
    Model: LargeLanguageModel,
    Output: DeserializeOwned + Send,
{
    let max_attempts = policy.max_attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=max_attempts {
        let permit = match governor {
            Some(governor) => Some(governor.acquire().await?),
            None => None,
        };
        let result = model.generate(request.clone()).await;
        drop(permit);

        match result {
            Ok(output) => return Ok(output),
            Err(error) if should_retry(&error) && attempt < max_attempts => {
                let advice = retry_advice(&error);
                let delay = retry_delay(&policy, attempt, advice, &request);
                warn!(
                    operation = ?request.operation,
                    attempt,
                    max_attempts,
                    delay_ms = delay.as_millis(),
                    error_kind = error.kind(),
                    "Retrying crawler LLM generation"
                );
                sleep(delay).await;
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(
        last_error.unwrap_or_else(|| LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after: None,
            },
            source: static_error("crawler LLM retry loop exhausted unexpectedly"),
        }),
    )
}

/// Run bounded response correction on top of bounded provider retry.
///
/// With three validation attempts and the default three transport attempts, one logical
/// operation makes at most nine provider calls.
pub async fn generate_validated_with_governor<Model, Wire, Output, E, Validate, Feedback>(
    model: &Model,
    governor: Option<&Arc<CrawlerLlmGovernor>>,
    base_request: StructuredGenerationRequest,
    max_validation_attempts: usize,
    validate: Validate,
    feedback: Feedback,
) -> Result<Output, ValidatedGenerationError<E>>
where
    Model: LargeLanguageModel,
    Wire: DeserializeOwned + Send,
    Output: Send,
    Validate: Fn(Wire) -> Result<Output, E>,
    Feedback: Fn(&E) -> &'static str,
{
    let max_attempts = max_validation_attempts.max(1);
    let mut request = base_request.clone();
    for attempt in 1..=max_attempts {
        let wire: Wire = match generate_with_governor(model, governor, request).await {
            Ok(wire) => wire,
            Err(error) if is_correctable_response_error(&error) && attempt < max_attempts => {
                let feedback_code = "response_not_valid_json";
                warn!(
                    operation = ?base_request.operation,
                    validation_attempt = attempt,
                    max_validation_attempts = max_attempts,
                    feedback_code,
                    "Retrying invalid structured LLM response"
                );
                request = correction_request(&base_request, attempt, feedback_code);
                continue;
            }
            Err(error) => return Err(ValidatedGenerationError::Model(error)),
        };

        match validate(wire) {
            Ok(output) => return Ok(output),
            Err(error) if attempt < max_attempts => {
                let feedback_code = feedback(&error);
                warn!(
                    operation = ?base_request.operation,
                    validation_attempt = attempt,
                    max_validation_attempts = max_attempts,
                    feedback_code,
                    "Retrying semantically invalid structured LLM response"
                );
                request = correction_request(&base_request, attempt, feedback_code);
            }
            Err(error) => return Err(ValidatedGenerationError::Validation(error)),
        }
    }

    Err(ValidatedGenerationError::Model(
        LargeLanguageModelError::InvalidResponse {
            source: static_error("validated LLM generation exhausted unexpectedly"),
        },
    ))
}

pub fn correction_request(
    base: &StructuredGenerationRequest,
    attempt: usize,
    feedback_code: &str,
) -> StructuredGenerationRequest {
    let mut request = base.clone();
    request.prompt.push_str(&format!(
        "\n\nThe previous response failed validation: {feedback_code}. Attempt {attempt} failed. Return a corrected response matching the supplied JSON Schema and all stated rules. Return JSON only."
    ));
    request
}

fn is_correctable_response_error(error: &LargeLanguageModelError) -> bool {
    matches!(error, LargeLanguageModelError::InvalidResponse { .. })
}

fn should_retry(error: &LargeLanguageModelError) -> bool {
    matches!(
        error,
        LargeLanguageModelError::Timeout { .. } | LargeLanguageModelError::Retryable { .. }
    )
}

fn retry_advice(error: &LargeLanguageModelError) -> LargeLanguageModelRetryAdvice {
    error
        .retry_advice()
        .unwrap_or(LargeLanguageModelRetryAdvice {
            kind: LargeLanguageModelRetryKind::Transient,
            retry_after: None,
        })
}

fn retry_delay(
    policy: &CrawlerLlmRetryPolicy,
    attempt: usize,
    advice: LargeLanguageModelRetryAdvice,
    request: &StructuredGenerationRequest,
) -> Duration {
    let (base, maximum) = match advice.kind {
        LargeLanguageModelRetryKind::RateLimited => {
            (policy.rate_limited_base, policy.rate_limited_max)
        }
        LargeLanguageModelRetryKind::ServiceUnavailable => (
            policy.service_unavailable_base,
            policy.service_unavailable_max,
        ),
        LargeLanguageModelRetryKind::Transient => (policy.transient_base, policy.transient_max),
    };
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let local = base
        .checked_mul(2_u32.saturating_pow(exponent))
        .unwrap_or(maximum)
        .min(maximum);
    let jittered = add_deterministic_jitter(local, policy.jitter_percent, request);
    advice
        .retry_after
        .map_or(jittered, |retry_after| retry_after.max(jittered))
}

fn add_deterministic_jitter(
    base: Duration,
    percent: u8,
    request: &StructuredGenerationRequest,
) -> Duration {
    if percent == 0 {
        return base;
    }
    let seed = request
        .operation
        .as_str()
        .bytes()
        .map(u64::from)
        .sum::<u64>()
        .saturating_add(request.prompt.len() as u64);
    let fraction = seed % 101;
    let jitter_nanos = base
        .as_nanos()
        .saturating_mul(u128::from(percent))
        .saturating_mul(u128::from(fraction))
        / 10_000;
    let jitter_nanos = u64::try_from(jitter_nanos).unwrap_or(u64::MAX);
    base.saturating_add(Duration::from_nanos(jitter_nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use large_language_model::LlmOperation;
    use serde::de::DeserializeOwned;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use tokio::task::JoinSet;

    fn request() -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object"}),
            options: large_language_model::GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(180),
            },
        }
    }

    struct StartRecordingModel {
        starts: Arc<StdMutex<Vec<Instant>>>,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for StartRecordingModel {
        async fn generate<Output>(
            &self,
            _: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            self.starts
                .lock()
                .map_err(|error| LargeLanguageModelError::InvalidResponse {
                    source: Box::new(std::io::Error::other(error.to_string())),
                })?
                .push(Instant::now());

            serde_json::from_value(serde_json::json!({ "ok": true })).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: Box::new(source),
                }
            })
        }
    }

    struct SequenceModel {
        responses: StdMutex<VecDeque<Result<serde_json::Value, LargeLanguageModelError>>>,
        calls: StdMutex<usize>,
    }

    impl SequenceModel {
        fn new(responses: Vec<Result<serde_json::Value, LargeLanguageModelError>>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                calls: StdMutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.lock().map(|calls| *calls).unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for SequenceModel {
        async fn generate<Output>(
            &self,
            _: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            if let Ok(mut calls) = self.calls.lock() {
                *calls += 1;
            }
            let response = self
                .responses
                .lock()
                .ok()
                .and_then(|mut responses| responses.pop_front())
                .unwrap_or_else(|| {
                    Err(LargeLanguageModelError::Permanent {
                        source: static_error("test response sequence exhausted"),
                    })
                })?;
            serde_json::from_value(response).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: Box::new(source),
                }
            })
        }
    }

    fn timeout_error() -> LargeLanguageModelError {
        LargeLanguageModelError::Timeout {
            source: static_error("timeout"),
        }
    }

    fn transient_error() -> LargeLanguageModelError {
        LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after: None,
            },
            source: static_error("transient"),
        }
    }

    fn policy() -> CrawlerLlmRetryPolicy {
        CrawlerLlmRetryPolicy {
            jitter_percent: 0,
            ..Default::default()
        }
    }

    #[test]
    fn should_calculate_provider_specific_retry_delays() {
        let request = request();
        let policy = policy();
        assert_eq!(
            retry_delay(
                &policy,
                1,
                LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::RateLimited,
                    retry_after: None,
                },
                &request
            ),
            Duration::from_secs(30)
        );
        assert_eq!(
            retry_delay(
                &policy,
                1,
                LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                    retry_after: None,
                },
                &request
            ),
            Duration::from_secs(600)
        );
        assert_eq!(
            retry_delay(
                &policy,
                2,
                LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::Transient,
                    retry_after: None,
                },
                &request
            ),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn should_not_shorten_provider_retry_after() {
        let policy = policy();
        let delay = retry_delay(
            &policy,
            1,
            LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after: Some(Duration::from_secs(8)),
            },
            &request(),
        );
        assert_eq!(delay, Duration::from_secs(8));
    }

    #[tokio::test(start_paused = true)]
    async fn should_serialize_request_starts_when_multiple_permits_are_available() {
        let min_interval = Duration::from_secs(2);
        let starts = Arc::new(StdMutex::new(Vec::new()));
        let model = Arc::new(StartRecordingModel {
            starts: Arc::clone(&starts),
        });
        let governor = Arc::new(CrawlerLlmGovernor::new(CrawlerLlmRateLimitConfig {
            max_concurrent_requests: 3,
            min_request_interval: min_interval,
        }));

        let mut tasks = JoinSet::new();
        for _ in 0..3 {
            let model = Arc::clone(&model);
            let governor = Arc::clone(&governor);
            tasks.spawn(async move {
                generate_with_governor::<_, serde_json::Value>(
                    model.as_ref(),
                    Some(&governor),
                    request(),
                )
                .await
            });
        }

        while let Some(result) = tasks.join_next().await {
            assert!(result.expect("request task must join").is_ok());
        }

        let mut starts = starts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        starts.sort();

        assert_eq!(starts.len(), 3);
        for adjacent in starts.windows(2) {
            assert!(
                adjacent[1].duration_since(adjacent[0]) >= min_interval,
                "request starts were not paced: {starts:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_release_start_gate_after_reserving_future_slots() {
        let min_interval = Duration::from_secs(2);
        let governor = Arc::new(CrawlerLlmGovernor::new(CrawlerLlmRateLimitConfig {
            max_concurrent_requests: 4,
            min_request_interval: min_interval,
        }));

        let first = governor.acquire().await.unwrap();
        let first_start = Instant::now();
        drop(first);

        let mut tasks = JoinSet::new();
        for _ in 0..3 {
            let governor = Arc::clone(&governor);
            tasks.spawn(async move {
                let permit = governor.acquire().await;
                drop(permit);
            });
        }

        let expected_next_start = first_start + min_interval * 4;
        for _ in 0..10 {
            let next = governor
                .next_request_start_at
                .try_lock()
                .expect("start gate must not be held while callers wait");
            if *next == Some(expected_next_start) {
                break;
            }
            drop(next);
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *governor.next_request_start_at.lock().await,
            Some(expected_next_start)
        );

        tokio::time::advance(min_interval * 3).await;
        while let Some(result) = tasks.join_next().await {
            result.expect("reservation task must join");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_retry_timeout_and_stop_after_policy_attempts() {
        let model = SequenceModel::new(vec![
            Err(timeout_error()),
            Err(timeout_error()),
            Err(timeout_error()),
        ]);
        let result = generate_with_governor_and_policy::<_, serde_json::Value>(
            &model,
            None,
            request(),
            policy(),
        )
        .await;
        assert!(matches!(
            result,
            Err(LargeLanguageModelError::Timeout { .. })
        ));
        assert_eq!(model.calls(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn should_release_governor_permit_during_backoff() {
        let model = Arc::new(SequenceModel::new(vec![
            Err(transient_error()),
            Ok(serde_json::json!({"ok": true})),
            Ok(serde_json::json!({"ok": true})),
        ]));
        let governor = Arc::new(CrawlerLlmGovernor::new(CrawlerLlmRateLimitConfig {
            max_concurrent_requests: 1,
            min_request_interval: Duration::ZERO,
        }));
        let first_model = Arc::clone(&model);
        let first_governor = Arc::clone(&governor);
        let first = tokio::spawn(async move {
            generate_with_governor_and_policy::<_, serde_json::Value>(
                first_model.as_ref(),
                Some(&first_governor),
                request(),
                policy(),
            )
            .await
        });
        while model.calls() < 1 {
            tokio::task::yield_now().await;
        }

        let second_model = Arc::clone(&model);
        let second_governor = Arc::clone(&governor);
        let second = tokio::spawn(async move {
            generate_with_governor_and_policy::<_, serde_json::Value>(
                second_model.as_ref(),
                Some(&second_governor),
                request(),
                policy(),
            )
            .await
        });

        assert!(second.await.unwrap().is_ok());
        assert!(first.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn should_retry_malformed_structured_response_with_bounded_feedback() {
        let model = SequenceModel::new(vec![
            Err(LargeLanguageModelError::InvalidResponse {
                source: static_error("malformed"),
            }),
            Ok(serde_json::json!({"ok": true})),
        ]);
        let output =
            generate_validated_with_governor::<_, serde_json::Value, serde_json::Value, _, _, _>(
                &model,
                None,
                request(),
                3,
                Result::<serde_json::Value, ()>::Ok,
                |_: &()| "invalid_output",
            )
            .await;
        assert_eq!(output.ok(), Some(serde_json::json!({"ok": true})));
        assert_eq!(model.calls(), 2);
    }
}
