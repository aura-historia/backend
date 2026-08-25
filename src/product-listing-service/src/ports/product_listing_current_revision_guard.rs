use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use std::collections::HashMap;

/// Exact expected current ProductListing revision for a batched invariant-critical check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingCurrentRevisionRef {
    pub product_listing_id: ProductListingId,
    pub expected_event_id: EventId,
}

/// Result of locking the current ProductListing revision for an invariant-critical write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingCurrentRevisionCheck {
    Current,
    Stale,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingCurrentRevisionCheckError {
    #[error("product current revision check failed")]
    CheckFailed {
        #[source]
        source: BoxError,
    },
}

/// Locks the authoritative ProductListing row until its surrounding transaction ends.
///
/// A `Current` result guarantees the expected event remains current until this
/// transaction commits or rolls back.
#[async_trait::async_trait]
pub trait ProductListingCurrentRevisionGuard: Send {
    async fn lock_and_check(
        &mut self,
        product_listing_id: ProductListingId,
        expected_event_id: EventId,
    ) -> Result<ProductListingCurrentRevisionCheck, ProductListingCurrentRevisionCheckError>;

    /// Locks all found ProductListing rows until the transaction ends and checks each ref.
    async fn lock_and_check_all(
        &mut self,
        refs: &[ProductListingCurrentRevisionRef],
    ) -> Result<
        HashMap<ProductListingCurrentRevisionRef, ProductListingCurrentRevisionCheck>,
        ProductListingCurrentRevisionCheckError,
    > {
        let mut checks = HashMap::new();
        for reference in refs {
            checks.insert(
                *reference,
                self.lock_and_check(reference.product_listing_id, reference.expected_event_id)
                    .await?,
            );
        }
        Ok(checks)
    }
}

pub trait ProductListingCurrentRevisionGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingCurrentRevisionGuard + 'tx;
}
