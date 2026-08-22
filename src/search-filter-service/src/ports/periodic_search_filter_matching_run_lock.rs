use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum PeriodicSearchFilterMatchingRunLockError {
    #[error("periodic search-filter matching run lock failed")]
    LockFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter matching run lock release failed")]
    ReleaseFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PeriodicSearchFilterMatchingRunLease: Send {
    async fn release(self: Box<Self>) -> Result<(), PeriodicSearchFilterMatchingRunLockError>;
}

#[async_trait::async_trait]
pub trait PeriodicSearchFilterMatchingRunLock: Send + Sync {
    async fn try_acquire(
        &self,
    ) -> Result<
        Option<Box<dyn PeriodicSearchFilterMatchingRunLease>>,
        PeriodicSearchFilterMatchingRunLockError,
    >;
}
