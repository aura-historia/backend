use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;

/// A listing and the event its projection is fenced on: the projector writes a listing only
/// for the event that is still its current event, so a full reprojection triggers each listing
/// with exactly that event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingProjectionTrigger {
    pub product_listing_id: ProductListingId,
    pub current_event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingProjectionTriggerReadError {
    #[error("product listing projection trigger query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted product listing projection trigger is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Pages every existing listing, active or withdrawn, in listing ID order.
#[async_trait::async_trait]
pub trait ProductListingProjectionTriggerReader: Send + Sync {
    /// The first `limit` listings after `after` (from the start when `None`).
    async fn page_after(
        &self,
        after: Option<ProductListingId>,
        limit: u32,
    ) -> Result<Vec<ProductListingProjectionTrigger>, ProductListingProjectionTriggerReadError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use std::error::Error;

    #[test]
    fn should_preserve_the_failure_source() {
        for error in [
            ProductListingProjectionTriggerReadError::QueryFailed {
                source: static_error("connection lost"),
            },
            ProductListingProjectionTriggerReadError::InvalidPersistedState {
                source: static_error("connection lost"),
            },
        ] {
            assert_eq!(
                Some("connection lost"),
                error.source().map(ToString::to_string).as_deref()
            );
        }
    }
}
