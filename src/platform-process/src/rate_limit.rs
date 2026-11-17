//! A process-local request-rate limit: a token bucket with a sustained rate and a burst. It
//! replaces API Gateway's stage and route throttles in the HTTP processes.

use std::{sync::Mutex, time::Instant};

/// A sustained rate and the burst admitted above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateLimit {
    per_second: f64,
    burst: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a rate limit needs a positive, finite rate and a burst of at least one request")]
pub struct InvalidRateLimit;

// Construction rejects NaN, so equality is reflexive.
impl Eq for RateLimit {}

impl RateLimit {
    pub fn new(per_second: f64, burst: f64) -> Result<Self, InvalidRateLimit> {
        if !(per_second.is_finite() && per_second > 0.0 && burst.is_finite() && burst >= 1.0) {
            return Err(InvalidRateLimit);
        }
        Ok(Self { per_second, burst })
    }
}

/// A token bucket over a [`RateLimit`]; it starts full.
pub struct TokenBucket {
    limit: RateLimit,
    state: Mutex<(f64, Instant)>,
}

impl TokenBucket {
    pub fn new(limit: RateLimit) -> Self {
        Self {
            limit,
            state: Mutex::new((limit.burst, Instant::now())),
        }
    }

    /// Takes one token when one is available.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    fn try_acquire_at(&self, now: Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (tokens, last) = *state;
        let refilled = now.saturating_duration_since(last).as_secs_f64() * self.limit.per_second;
        let tokens = (tokens + refilled).min(self.limit.burst);
        let (tokens, admitted) = if tokens >= 1.0 {
            (tokens - 1.0, true)
        } else {
            (tokens, false)
        };
        *state = (tokens, now.max(last));
        admitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn limit(per_second: f64, burst: f64) -> RateLimit {
        RateLimit::new(per_second, burst).unwrap()
    }

    #[test]
    fn should_admit_a_burst_then_refill_at_the_sustained_rate() {
        let bucket = TokenBucket::new(limit(2.0, 3.0));
        let start = bucket.state.lock().unwrap().1;

        let burst: Vec<bool> = (0..4).map(|_| bucket.try_acquire_at(start)).collect();
        assert_eq!(vec![true, true, true, false], burst);
        assert!(!bucket.try_acquire_at(start + Duration::from_millis(400)));
        assert!(bucket.try_acquire_at(start + Duration::from_millis(500)));
        assert!(
            (0..3).all(|_| bucket.try_acquire_at(start + Duration::from_secs(60)))
                && !bucket.try_acquire_at(start + Duration::from_secs(60)),
            "refill stops at the burst"
        );
    }

    #[test]
    fn should_reject_limits_that_admit_nothing() {
        for (per_second, burst) in [(0.0, 1.0), (-1.0, 1.0), (f64::NAN, 1.0), (1.0, 0.5)] {
            assert_eq!(Err(InvalidRateLimit), RateLimit::new(per_second, burst));
        }
    }
}
