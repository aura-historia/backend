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
