use common::error::boxed::BoxError;
use common::event_id::EventId;
use common::product_id::ProductId;

/// Result of locking the current Product revision for an invariant-critical write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductCurrentRevisionCheck {
    Current,
    Stale,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductCurrentRevisionCheckError {
    #[error("product current revision check failed")]
    CheckFailed {
        #[source]
        source: BoxError,
    },
}

/// Locks the authoritative Product row until its surrounding transaction ends.
///
/// A `Current` result guarantees the expected event remains current until this
/// transaction commits or rolls back.
#[async_trait::async_trait]
pub trait ProductCurrentRevisionGuard: Send {
    async fn lock_and_check(
        &mut self,
        product_id: ProductId,
        expected_event_id: EventId,
    ) -> Result<ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError>;
}

pub trait ProductCurrentRevisionGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductCurrentRevisionGuard + 'tx;
}
