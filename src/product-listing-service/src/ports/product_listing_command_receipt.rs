use application::error::BoxError;
use listing_source_core::ListingSourceId;
use time::OffsetDateTime;

use crate::use_cases::commands::product_listing_ingestion::{
    ProductListingIngestionFingerprint, ProductListingIngestionOperation,
};

/// Only successfully applied commands have durable execution receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingCommandCompletionCode {
    Applied,
}

impl ProductListingCommandCompletionCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "APPLIED",
        }
    }
}

/// Committed successful execution. Submission ID is metadata only; command ID is the lookup key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingCommandReceipt {
    pub submission_id: String,
    pub listing_source_id: ListingSourceId,
    pub operation: ProductListingIngestionOperation,
    pub fingerprint: ProductListingIngestionFingerprint,
    pub completion_code: ProductListingCommandCompletionCode,
    pub completed_at: OffsetDateTime,
}

/// The consumer supplies the verified wire identifiers via `as_str()`; the adapter validates
/// their versioned shape independently before persisting them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingCommandReceiptWrite {
    pub command_id: String,
    pub submission_id: String,
    pub listing_source_id: ListingSourceId,
    pub operation: ProductListingIngestionOperation,
    pub fingerprint: ProductListingIngestionFingerprint,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingCommandReceiptError {
    #[error("invalid product listing command receipt identity")]
    InvalidIdentity,
    #[error("product listing command receipt already exists")]
    AlreadyExists,
    #[error("persisted product listing command receipt is invalid")]
    InvalidPersistedState,
    #[error("product listing command receipt operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
}

/// Use in the same unit of work as the canonical write. Lock before looking up the command;
/// hold the transaction through the canonical write and receipt insert/commit. Never treat a
/// missing receipt as permission to execute outside that transaction. Insert only after a
/// successful execution; a business rejection must not create a receipt.
#[async_trait::async_trait]
pub trait ProductListingCommandReceiptStore: Send {
    async fn lock_and_find(
        &mut self,
        command_id: &str,
    ) -> Result<Option<ProductListingCommandReceipt>, ProductListingCommandReceiptError>;

    async fn insert(
        &mut self,
        receipt: &ProductListingCommandReceiptWrite,
    ) -> Result<(), ProductListingCommandReceiptError>;
}

pub trait ProductListingCommandReceiptStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingCommandReceiptStore + 'tx;
}
