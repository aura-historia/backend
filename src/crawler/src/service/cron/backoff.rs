use std::time::Duration;

pub(super) const MAX_ADAPTIVE_DOMAIN_DELAY: Duration = Duration::from_secs(60);
pub(super) const ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY: usize = 5;

fn next_adaptive_domain_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_ADAPTIVE_DOMAIN_DELAY)
}

pub(super) struct AdaptiveDomainBackoff {
    base_delay: Duration,
    current_delay: Duration,
    consecutive_successes: usize,
}

impl AdaptiveDomainBackoff {
    pub(super) fn new(base_delay: Duration) -> Self {
        Self {
            base_delay,
            current_delay: base_delay,
            consecutive_successes: 0,
        }
    }

    pub(super) fn current_delay(&self) -> Duration {
        self.current_delay
    }

    pub(super) fn record_retryable_failure(&mut self) -> (Duration, Duration) {
        self.consecutive_successes = 0;
        let previous_delay = self.current_delay;
        self.current_delay = next_adaptive_domain_delay(self.current_delay);
        (previous_delay, self.current_delay)
    }

    pub(super) fn record_clean_outcome(&mut self) -> Option<(Duration, Duration)> {
        if self.current_delay == self.base_delay {
            self.consecutive_successes = 0;
            return None;
        }

        self.consecutive_successes += 1;
        if self.consecutive_successes < ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY {
            return None;
        }

        self.consecutive_successes = 0;
        let previous_delay = self.current_delay;
        self.current_delay = (self.current_delay / 2).max(self.base_delay);
        Some((previous_delay, self.current_delay))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_double_adaptive_domain_delay_until_cap() {
        let mut backoff = AdaptiveDomainBackoff::new(Duration::from_secs(1));

        assert_eq!(
            backoff.record_retryable_failure(),
            (Duration::from_secs(1), Duration::from_secs(2))
        );

        let mut backoff = AdaptiveDomainBackoff {
            base_delay: Duration::from_secs(1),
            current_delay: Duration::from_secs(45),
            consecutive_successes: 0,
        };
        assert_eq!(
            backoff.record_retryable_failure(),
            (Duration::from_secs(45), MAX_ADAPTIVE_DOMAIN_DELAY)
        );
    }

    #[test]
    fn should_hold_adaptive_domain_delay_until_sustained_successes() {
        let mut backoff = AdaptiveDomainBackoff {
            base_delay: Duration::from_secs(1),
            current_delay: Duration::from_secs(8),
            consecutive_successes: 0,
        };

        for _ in 0..ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY - 1 {
            assert_eq!(backoff.record_clean_outcome(), None);
            assert_eq!(backoff.current_delay(), Duration::from_secs(8));
        }

        assert_eq!(
            backoff.record_clean_outcome(),
            Some((Duration::from_secs(8), Duration::from_secs(4)))
        );
        assert_eq!(backoff.current_delay(), Duration::from_secs(4));
    }

    #[test]
    fn should_not_decay_adaptive_domain_delay_below_base() {
        let mut backoff = AdaptiveDomainBackoff {
            base_delay: Duration::from_secs(1),
            current_delay: Duration::from_secs(2),
            consecutive_successes: ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY - 1,
        };

        assert_eq!(
            backoff.record_clean_outcome(),
            Some((Duration::from_secs(2), Duration::from_secs(1)))
        );
        assert_eq!(backoff.record_clean_outcome(), None);
        assert_eq!(backoff.current_delay(), Duration::from_secs(1));
    }

    #[test]
    fn should_reset_success_counter_after_retryable_network_failure() {
        let mut backoff = AdaptiveDomainBackoff {
            base_delay: Duration::from_secs(1),
            current_delay: Duration::from_secs(4),
            consecutive_successes: ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY - 1,
        };

        assert_eq!(
            backoff.record_retryable_failure(),
            (Duration::from_secs(4), Duration::from_secs(8))
        );

        for _ in 0..ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY - 1 {
            assert_eq!(backoff.record_clean_outcome(), None);
            assert_eq!(backoff.current_delay(), Duration::from_secs(8));
        }
    }
}
