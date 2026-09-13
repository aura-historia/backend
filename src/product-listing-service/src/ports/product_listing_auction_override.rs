use super::product_listing_raw_capture_writer::ProductListingRawStreamId;
use application::error::BoxError;
use async_trait::async_trait;
use auction_core::AuctionId;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;

/// Independent optimistic token for listing-owned Auction-ingestion policy. Zero means the
/// policy row is absent; persisted rows begin at one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ProductListingAuctionPolicyVersion(u64);

impl ProductListingAuctionPolicyVersion {
    pub const fn into_inner(self) -> u64 {
        self.0
    }
}

impl From<u64> for ProductListingAuctionPolicyVersion {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Restricted audit input. The reason must never enter ordinary listing events or reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingAuctionOverrideAudit {
    pub audit_id: EventId,
    pub product_listing_id: ProductListingId,
    pub actor_label: String,
    pub reason: String,
    pub previous_auction_id: Option<AuctionId>,
    pub current_auction_id: Option<AuctionId>,
    pub recorded_at: OffsetDateTime,
}

/// Listing-owned policy state. It is intentionally separate from ProductListing facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingAuctionOverride {
    pub version: ProductListingAuctionPolicyVersion,
    pub active: bool,
    /// Global immutable raw-capture sequence fence reserved on release. Revisions captured at or
    /// before this fence cannot apply Auction facts, even if a stream is linked later.
    pub release_capture_generation_fence: Option<u64>,
}

/// Immutable raw capture identity used only for raw Auction-context admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingRawAuctionCapture {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub revision: u64,
    pub capture_generation: u64,
}

/// Result of an Auction-context admission decision for one immutable raw revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingRawAuctionContextAdmission {
    Admit,
    Preserve,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingAuctionOverrideError {
    #[error("auction override policy concurrency conflict")]
    ConcurrencyConflict,
    #[error("auction override policy release cannot establish a safe raw-observation floor")]
    UnsafeRelease,
    #[error("auction override policy persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("auction override policy persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait ProductListingAuctionOverrideRepository: Send {
    /// Serializes every Auction-context policy decision for one listing until the caller-owned
    /// transaction ends. It protects both persisted and absent policy rows.
    async fn lock(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<(), ProductListingAuctionOverrideError>;

    async fn find(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingAuctionOverride>, ProductListingAuctionOverrideError>;

    /// Admits or preserves one raw Auction-context assertion. The caller must first hold
    /// [`Self::lock`] for `product_listing_id` in the same transaction. The adapter validates the
    /// immutable raw identity and enforces both the release fence and any linked-stream floor.
    async fn admit_raw_auction_context(
        &mut self,
        product_listing_id: ProductListingId,
        raw_capture: ProductListingRawAuctionCapture,
    ) -> Result<ProductListingRawAuctionContextAdmission, ProductListingAuctionOverrideError>;

    /// Activates the policy and stores restricted correction audit information. The absent row
    /// has policy version zero; a successful activation creates version one.
    async fn activate(
        &mut self,
        audit: &ProductListingAuctionOverrideAudit,
        expected_version: ProductListingAuctionPolicyVersion,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError>;

    /// Releases an active policy by reserving a raw-capture sequence fence and recording each
    /// currently linked stream's newest pre-fence revision. It never changes ProductListing facts.
    async fn release(
        &mut self,
        product_listing_id: ProductListingId,
        expected_version: ProductListingAuctionPolicyVersion,
        audit_id: EventId,
        actor_label: String,
        recorded_at: OffsetDateTime,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError>;
}

pub trait ProductListingAuctionOverrideRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingAuctionOverrideRepository + 'tx;
}

#[cfg(test)]
mod tests {
    use super::{ProductListingAuctionPolicyVersion, ProductListingRawAuctionContextAdmission};

    #[test]
    fn should_represent_an_absent_policy_row_with_version_zero() {
        assert_eq!(
            0,
            ProductListingAuctionPolicyVersion::default().into_inner()
        );
    }

    #[test]
    fn should_keep_raw_auction_admission_outcomes_explicit() {
        assert_ne!(
            ProductListingRawAuctionContextAdmission::Admit,
            ProductListingRawAuctionContextAdmission::Preserve
        );
    }
}
