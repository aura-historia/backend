use crate::use_cases::commands::product_listing_ingestion::{
    ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
};
use application::error::BoxError;

/// Publishes service-prepared product-listing ingestion commands.
///
/// Implementations must return exactly one item outcome for every supplied command. For commands
/// in the same logical FIFO group, they preserve input order, have at most one send outstanding,
/// and wait for acceptance confirmation before sending the successor. An unconfirmed or failed
/// eligible predecessor leaves its successors `NotAttempted`; unrelated groups may progress
/// concurrently within the publisher's budget. If sending has started, transport failures are item
/// outcomes (`Rejected`, `Unconfirmed`, or `NotAttempted`), not this whole-call error. Errors from
/// this method mean that no command was attempted.
#[async_trait::async_trait]
pub trait ProductListingIngestionPublisher: Send + Sync {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>;
}

#[derive(Debug, thiserror::Error)]
#[error("product listing ingestion publisher could not start the submission")]
pub struct ProductListingIngestionPublishError {
    #[source]
    pub source: BoxError,
}

impl ProductListingIngestionPublishError {
    pub fn new(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            source: Box::new(source),
        }
    }
}
