use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_core::product_id::ProductId;
use std::collections::HashMap;

/// Exact expected current Product revision for a batched invariant-critical check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductCurrentRevisionRef {
    pub product_id: ProductId,
    pub expected_event_id: EventId,
}

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

    /// Locks all found Product rows until the transaction ends and checks each ref.
    async fn lock_and_check_all(
        &mut self,
        refs: &[ProductCurrentRevisionRef],
    ) -> Result<
        HashMap<ProductCurrentRevisionRef, ProductCurrentRevisionCheck>,
        ProductCurrentRevisionCheckError,
    > {
        let mut checks = HashMap::new();
        for reference in refs {
            checks.insert(
                *reference,
                self.lock_and_check(reference.product_id, reference.expected_event_id)
                    .await?,
            );
        }
        Ok(checks)
    }
}

pub trait ProductCurrentRevisionGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductCurrentRevisionGuard + 'tx;
}
