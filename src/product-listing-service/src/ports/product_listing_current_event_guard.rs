use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingCurrentEventCheckError {
    #[error("product current event check failed")]
    CheckFailed {
        #[source]
        source: BoxError,
    },
}

/// Locks authoritative ProductListing rows until their surrounding transaction ends.
///
/// The returned current events stay current until this transaction commits or rolls back, so
/// reads in the same transaction see the listing state they name.
#[async_trait::async_trait]
pub trait ProductListingCurrentEventGuard: Send {
    /// Locks the found listings and returns each one's current event; missing listings are absent.
    async fn lock_current_events(
        &mut self,
        product_listing_ids: &[ProductListingId],
    ) -> Result<HashMap<ProductListingId, EventId>, ProductListingCurrentEventCheckError>;
}

pub trait ProductListingCurrentEventGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingCurrentEventGuard + 'tx;
}
