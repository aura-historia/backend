use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct PublicListingSourceReadBudget {
    permits: Arc<Semaphore>,
    request_timeout: Duration,
}

impl PublicListingSourceReadBudget {
    pub fn new(max_in_flight: usize, request_timeout: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_in_flight)),
            request_timeout,
        }
    }

    pub fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    pub const fn request_timeout(&self) -> Duration {
        self.request_timeout
    }
}

impl Default for PublicListingSourceReadBudget {
    fn default() -> Self {
        Self::new(4, Duration::from_millis(500))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_to_the_existing_public_read_policy() {
        let budget = PublicListingSourceReadBudget::default();
        assert_eq!(Duration::from_millis(500), budget.request_timeout());
        let permits = (0..4)
            .map(|_| budget.try_acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(budget.try_acquire().is_none());
        drop(permits);
        assert!(budget.try_acquire().is_some());
    }
}
