use std::collections::HashSet;
use std::fmt::Display;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl RetryConfig {
    pub const fn new(max_attempts: u32, initial_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            max_attempts,
            initial_backoff,
            max_backoff,
        }
    }

    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub const fn initial_backoff(&self) -> Duration {
        self.initial_backoff
    }

    pub const fn max_backoff(&self) -> Duration {
        self.max_backoff
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::new(3, Duration::from_millis(100), Duration::from_secs(5))
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryDeadLetterQueue<T> {
    entries: Arc<Mutex<Vec<DeadLetter<T>>>>,
}

impl<T> Default for InMemoryDeadLetterQueue<T> {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<T> InMemoryDeadLetterQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn push(&self, job: T, reason: String, attempts: u32) {
        self.entries.lock().await.push(DeadLetter {
            job,
            reason,
            attempts,
        });
    }

    pub async fn entries(&self) -> Vec<DeadLetter<T>>
    where
        T: Clone,
    {
        self.entries.lock().await.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetter<T> {
    pub job: T,
    pub reason: String,
    pub attempts: u32,
}

pub async fn run_with_retry<T, R, F, Fut, E>(
    job: T,
    config: RetryConfig,
    dead_letters: &InMemoryDeadLetterQueue<T>,
    mut handler: F,
) -> Result<R, RetryError<E>>
where
    T: Clone,
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<R, E>>,
    E: Display,
{
    let max_attempts = config.max_attempts().max(1);
    let mut attempt = 1;
    let mut backoff = config.initial_backoff();

    loop {
        match handler(job.clone()).await {
            Ok(result) => return Ok(result),
            Err(error) if attempt >= max_attempts => {
                let reason = error.to_string();
                warn!(attempts = attempt, %reason, "job retries exhausted; moving to in-memory DLQ");
                dead_letters.push(job, reason, attempt).await;
                return Err(RetryError::Exhausted {
                    source: error,
                    attempts: attempt,
                });
            }
            Err(error) => {
                warn!(attempt, error = %error, "job attempt failed; retrying in process");
                if !backoff.is_zero() {
                    tokio::time::sleep(backoff).await;
                }
                backoff = next_backoff(backoff, config.max_backoff());
                attempt += 1;
            }
        }
    }
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > max { max } else { doubled }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum RetryError<E> {
    #[error("job retries exhausted after {attempts} attempts")]
    Exhausted { source: E, attempts: u32 },
}

#[derive(Debug, Clone)]
pub struct InProcessIdempotencyStore<K> {
    processed: Arc<Mutex<HashSet<K>>>,
}

impl<K> Default for InProcessIdempotencyStore<K> {
    fn default() -> Self {
        Self {
            processed: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

impl<K> InProcessIdempotencyStore<K>
where
    K: Eq + Hash + Clone,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn is_processed(&self, key: &K) -> bool {
        self.processed.lock().await.contains(key)
    }

    pub async fn record_processed(&self, key: K) -> bool {
        self.processed.lock().await.insert(key)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn should_retry_until_job_succeeds() -> Result<(), Box<dyn std::error::Error>> {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_handler = attempts.clone();
        let dead_letters = InMemoryDeadLetterQueue::new();
        let config = RetryConfig::new(3, Duration::ZERO, Duration::ZERO);

        let result = run_with_retry("job-1".to_owned(), config, &dead_letters, move |_job| {
            let attempts_for_future = attempts_for_handler.clone();
            async move {
                let next_attempt = attempts_for_future.fetch_add(1, Ordering::SeqCst) + 1;
                if next_attempt < 2 {
                    Err("transient")
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(2, attempts.load(Ordering::SeqCst));
        assert!(dead_letters.entries().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_move_job_to_in_memory_dlq_when_retries_are_exhausted() {
        let dead_letters = InMemoryDeadLetterQueue::new();
        let config = RetryConfig::new(2, Duration::ZERO, Duration::ZERO);

        let result = run_with_retry("job-1".to_owned(), config, &dead_letters, |_job| async {
            Err::<(), _>("permanent")
        })
        .await;

        assert!(matches!(
            result,
            Err(RetryError::Exhausted {
                source: "permanent",
                attempts: 2,
            })
        ));
        assert_eq!(
            vec![DeadLetter {
                job: "job-1".to_owned(),
                reason: "permanent".to_owned(),
                attempts: 2,
            }],
            dead_letters.entries().await
        );
    }

    #[tokio::test]
    async fn should_track_processed_idempotency_keys() {
        let store = InProcessIdempotencyStore::new();
        let key = "product-event:40000000-0000-0000-0000-000000000001".to_owned();

        assert!(!store.is_processed(&key).await);
        assert!(store.record_processed(key.clone()).await);
        assert!(store.is_processed(&key).await);
        assert!(!store.record_processed(key).await);
    }
}
