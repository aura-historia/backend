//! Durable, source-authorized shipping evidence custody and canonical application.
pub mod canonical;
pub mod ports;
pub mod use_cases;

use application::{error::BoxError, transaction::TransactionError};
use listing_source_core::ListingSourceId;
use product_listing_core::{delivery::DeliveryIntegration, source_listing_id::SourceListingId};
use shipping_core::{
    inputs::ListingRateInputs,
    profile::{ShippingProfileContent, ShippingProfileId},
    selection::{DeliveryOverrides, ProfileSelection},
};
use shipping_service::ports::ShippingEvidence;
use time::OffsetDateTime;
use uuid::Uuid;

pub const MAX_CHUNKS: u32 = 16;
pub const MAX_CHUNK_RECORDS: usize = 16;
pub const MAX_RECORDS: u32 = 256;
pub const MAX_CHUNK_BYTES: usize = 65_536;
pub const MAX_OPEN_SESSIONS: i64 = 16;
pub const SESSION_HOURS: i64 = 24;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExternalKey(String);
impl ExternalKey {
    pub fn new(value: String) -> Result<Self, SyncError> {
        if value.trim().is_empty()
            || value.trim() != value
            || value.len() > 512
            || value.chars().any(char::is_control)
        {
            return Err(SyncError::InvalidInput);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservationWindow {
    pub observed_at: OffsetDateTime,
    pub fresh_until: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}
impl ObservationWindow {
    pub fn validate(self, now: OffsetDateTime) -> Result<(), SyncError> {
        if self.observed_at > now
            || self.fresh_until < self.observed_at
            || self.expires_at < self.fresh_until
            || self.expires_at <= now
        {
            return Err(SyncError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceSelection {
    Unresolved,
    Unavailable,
    Profile(ExternalKey),
    InheritSourceDefault,
    SuppressInheritance,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ItemAssertion {
    pub selection: EvidenceSelection,
    pub rate_inputs: ListingRateInputs,
    pub overrides: DeliveryOverrides,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Profile,
    Default,
    Item,
}
impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "PROFILE",
            Self::Default => "DEFAULT",
            Self::Item => "ITEM",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Assertion {
    Profile {
        content: Box<ShippingProfileContent>,
        evidence: Vec<ShippingEvidence>,
    },
    Default {
        profile: Option<ExternalKey>,
    },
    Item {
        source_listing_id: SourceListingId,
        assertion: Box<ItemAssertion>,
    },
    Delete {
        kind: ObjectKind,
    },
    /// Failed or missing extraction preserves assertions and their original freshness.
    Unavailable {
        capability: Capability,
        reason: UnavailableReason,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Configuration,
    Membership,
    RateInputs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    PermissionDenied,
    Unsupported,
    ExtractionFailed,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub key: ExternalKey,
    pub window: ObservationWindow,
    pub assertion: Assertion,
    /// Allowlisted acquisition identities, not provider payloads or executable settings.
    pub objects: Vec<ObservedObject>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedObject {
    pub kind: ObservedObjectKind,
    pub id: ExternalKey,
    pub parent: Option<ExternalKey>,
    pub profile: Option<ExternalKey>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservedObjectKind {
    Profile,
    Zone,
    Method,
    Class,
    Product,
    Variant,
}
impl Record {
    pub fn kind(&self) -> Option<ObjectKind> {
        match self.assertion {
            Assertion::Profile { .. } => Some(ObjectKind::Profile),
            Assertion::Default { .. } => Some(ObjectKind::Default),
            Assertion::Item { .. } => Some(ObjectKind::Item),
            Assertion::Delete { kind } => Some(kind),
            Assertion::Unavailable { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Snapshot,
    Delta,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "SNAPSHOT",
            Self::Delta => "DELTA",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Begin {
    pub epoch: Uuid,
    pub key: ExternalKey,
    pub scope: ExternalKey,
    pub mode: Mode,
    pub expected_generation: i64,
    pub expected_sequence: i64,
    pub provider_version: ExternalKey,
    /// Explicit completeness scope: profiles, source default, item membership/inputs.
    pub components: [bool; 3],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub chunks: u32,
    pub records: u32,
    /// SHA-256 of the concatenation of the ordered, fixed-width chunk digests.
    pub digest: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub source: ListingSourceId,
    pub epoch: Uuid,
    pub revision: i64,
    pub integration: DeliveryIntegration,
    pub active: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationState {
    Open,
    Ready,
    WaitingDependency,
    Applied,
    ReconcileRequired,
    Expired,
}
impl ApplicationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Ready => "READY",
            Self::WaitingDependency => "WAITING_DEPENDENCY",
            Self::Applied => "APPLIED",
            Self::ReconcileRequired => "RECONCILE_REQUIRED",
            Self::Expired => "EXPIRED",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub id: Uuid,
    pub source: ListingSourceId,
    pub begin: Begin,
    pub state: ApplicationState,
    pub raw_complete: bool,
    pub captured_chunks: u32,
    pub captured_records: u32,
    pub chunk_digests: Vec<(u32, [u8; 32])>,
    pub reconciliation_required: bool,
    pub generation: Option<i64>,
    pub failure: Option<String>,
    pub manifest: Option<Manifest>,
    pub started_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub digest: [u8; 32],
    pub records: Vec<Record>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeHead {
    pub generation: i64,
    pub sequence: i64,
    pub dirty: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemOwnership {
    pub current: [bool; 4],
    pub reclaimable: [bool; 4],
}

/// Last values owned by this acquisition source. A different current component belongs
/// to an explicit manager and cannot be adopted by a later refresh or tombstone.
#[derive(Clone, Debug, PartialEq)]
pub enum AppliedValue {
    Profile {
        id: ShippingProfileId,
        revision: u64,
    },
    Default {
        revision: u64,
        profile: Option<ShippingProfileId>,
    },
    Item {
        source_listing_id: SourceListingId,
        delivery: Box<product_listing_core::delivery::ListingDelivery>,
        owned: [bool; 4],
    },
    Tombstone,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub kind: ObjectKind,
    pub key: ExternalKey,
    pub scope: ExternalKey,
    pub window: ObservationWindow,
    pub value: AppliedValue,
    pub tombstoned: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("invalid shipping synchronization input")]
    InvalidInput,
    #[error("shipping synchronization retry identity conflicts")]
    Conflict,
    #[error("shipping synchronization epoch is no longer authoritative")]
    Revoked,
    #[error("shipping synchronization session not found")]
    NotFound,
    #[error("shipping snapshot is incomplete")]
    Incomplete,
    #[error("shipping synchronization dependency unavailable: {0}")]
    Dependency(&'static str),
    #[error("shipping synchronization scope needs reconciliation")]
    Dirty,
    #[error("shipping synchronization admission limit reached")]
    Limit,
    #[error("shipping synchronization authorization failed")]
    Authorization(
        #[from]
        #[source]
        shipping_service::ShippingError,
    ),
    #[error("shipping synchronization application failed")]
    Listing(
        #[from]
        #[source]
        product_listing_service::use_cases::ProductListingDeliveryError,
    ),
    #[error("shipping synchronization transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
    #[error("shipping synchronization storage failed")]
    Storage {
        #[source]
        source: BoxError,
    },
}

pub fn failure(source: impl std::error::Error + Send + Sync + 'static) -> SyncError {
    SyncError::Storage {
        source: Box::new(source),
    }
}

pub(crate) fn selected(
    value: &EvidenceSelection,
    profile: Option<ShippingProfileId>,
) -> Option<ProfileSelection> {
    match value {
        EvidenceSelection::Unresolved | EvidenceSelection::Unavailable => None,
        EvidenceSelection::Profile(_) => profile.map(ProfileSelection::Explicit),
        EvidenceSelection::InheritSourceDefault => Some(ProfileSelection::InheritSourceDefault),
        EvidenceSelection::SuppressInheritance => Some(ProfileSelection::SuppressInheritance),
    }
}
