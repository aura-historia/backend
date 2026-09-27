use crate::use_cases::{
    CaptureProductListingRawObservationCommand, CreateProductListingCommand,
    UpdateProductListingCommand, UpsertProductListingCommand,
};
use application::operation_context::{CorrelationId, Principal, RequestId};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use std::fmt::{Display, Formatter};
use user_core::user_id::UserId;

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

/// The service-owned identity of the operation being admitted to asynchronous ingestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductListingIngestionOperation {
    Create,
    Update,
    Upsert,
    Withdraw,
    CaptureRaw,
}

impl ProductListingIngestionOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Update => "UPDATE",
            Self::Upsert => "UPSERT",
            Self::Withdraw => "WITHDRAW",
            Self::CaptureRaw => "CAPTURE_RAW",
        }
    }
}

/// A typed command at the async-ingestion boundary. Provider names are not operation variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingIngestionIntent {
    Create(CreateProductListingCommand),
    Update {
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    },
    Upsert(UpsertProductListingCommand),
    Withdraw(ProductListingKey),
    CaptureRaw(CaptureProductListingRawObservationCommand),
}

impl ProductListingIngestionIntent {
    pub const fn operation(&self) -> ProductListingIngestionOperation {
        match self {
            Self::Create(_) => ProductListingIngestionOperation::Create,
            Self::Update { .. } => ProductListingIngestionOperation::Update,
            Self::Upsert(_) => ProductListingIngestionOperation::Upsert,
            Self::Withdraw(_) => ProductListingIngestionOperation::Withdraw,
            Self::CaptureRaw(_) => ProductListingIngestionOperation::CaptureRaw,
        }
    }

    pub fn listing_source_id(&self) -> ListingSourceId {
        match self {
            Self::Create(command) => command.listing_source_id,
            Self::Update { product_key, .. } | Self::Withdraw(product_key) => {
                product_key.listing_source_id
            }
            Self::Upsert(command) => command.listing_source_id,
            Self::CaptureRaw(command) => command.listing_source_id,
        }
    }

    pub fn source_listing_id(&self) -> Option<&SourceListingId> {
        match self {
            Self::Create(command) => Some(&command.source_listing_id),
            Self::Update { product_key, .. } | Self::Withdraw(product_key) => {
                Some(&product_key.source_listing_id)
            }
            Self::Upsert(command) => Some(&command.source_listing_id),
            Self::CaptureRaw(_) => None,
        }
    }
}

/// An optional client-provided REST idempotency key. It is metadata, not business input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductListingIngestionIdempotencyKey(String);

/// Validation error for an invalid or oversized idempotency key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProductListingIngestionIdempotencyKeyError {
    #[error("idempotency key must contain 1 to 128 visible ASCII bytes")]
    Invalid,
}

impl ProductListingIngestionIdempotencyKey {
    pub fn new(
        value: impl Into<String>,
    ) -> Result<Self, ProductListingIngestionIdempotencyKeyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(ProductListingIngestionIdempotencyKeyError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn generated() -> Self {
        Self(format!("generated-{}", uuid::Uuid::new_v4()))
    }
}

impl Display for ProductListingIngestionIdempotencyKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable, opaque metadata joining commands admitted by one logical submission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductListingIngestionSubmissionId(String);

impl ProductListingIngestionSubmissionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(format!("plis1_{}", lowercase_hex(&digest)))
    }
}

impl Display for ProductListingIngestionSubmissionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable, opaque per-command identity used by downstream durable execution receipts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductListingIngestionCommandId(String);

impl ProductListingIngestionCommandId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(format!("plic1_{}", lowercase_hex(&digest)))
    }
}

impl Display for ProductListingIngestionCommandId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

/// Identity of the trusted caller, without authentication credentials or delegated tokens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProductListingIngestionActor {
    User(UserId),
    DelegatedUser(UserId),
    Service(String),
    System,
}

impl ProductListingIngestionActor {
    pub(crate) fn from_principal(principal: &Principal) -> Option<Self> {
        match principal {
            Principal::Anonymous => None,
            Principal::User(user_id) => Some(Self::User(*user_id)),
            Principal::DelegatedUser { user_id, .. } => Some(Self::DelegatedUser(*user_id)),
            Principal::Service(service_id) => Some(Self::Service(service_id.clone())),
            Principal::System => Some(Self::System),
        }
    }

    pub const fn principal_kind(&self) -> &'static str {
        match self {
            Self::User(_) => "USER",
            Self::DelegatedUser(_) => "DELEGATED_USER",
            Self::Service(_) => "SERVICE",
            Self::System => "SYSTEM",
        }
    }

    pub fn actor_id(&self) -> Option<String> {
        match self {
            Self::User(user_id) | Self::DelegatedUser(user_id) => Some(user_id.to_string()),
            Self::Service(service_id) => Some(service_id.clone()),
            Self::System => None,
        }
    }

    pub(crate) const fn identity_kind(&self) -> &'static str {
        match self {
            Self::User(_) | Self::DelegatedUser(_) => "USER",
            Self::Service(_) => "SERVICE",
            Self::System => "SYSTEM",
        }
    }

    pub(crate) fn identity_id(&self) -> Option<String> {
        self.actor_id()
    }
}

/// Trusted queue metadata. Request IDs are observability metadata and are excluded from identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingIngestionMetadata {
    pub submission_id: ProductListingIngestionSubmissionId,
    pub command_id: ProductListingIngestionCommandId,
    pub index: usize,
    pub input_count: usize,
    pub listing_source_id: ListingSourceId,
    pub operation: ProductListingIngestionOperation,
    pub actor: ProductListingIngestionActor,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}

/// One prepared command passed to the shared publisher port.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingIngestionMessage {
    pub metadata: ProductListingIngestionMetadata,
    pub intent: ProductListingIngestionIntent,
}

/// Why an item was rejected before publication or definitively rejected by a publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductListingIngestionRejectionReason {
    ListingSourceMismatch,
    InvalidAuctionPatch,
    UrlCannotBeCleared,
    SourceRecordKeyTooLong {
        len: usize,
        max: usize,
    },
    SourceRecordKeyEmbeddedNul,
    InvalidNormalizationInput,
    /// A stable, safe code from the publisher, never an exception string or raw payload.
    Publisher {
        code: String,
    },
}

/// Stable reason why a publisher was unable to attempt an eligible command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingIngestionNotAttemptedReason {
    DeadlineExceeded,
    BlockedByFifoPredecessor,
}

/// Per-item result. `Accepted` means queue acceptance only, not business completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductListingIngestionOutcome {
    Accepted,
    Rejected {
        reason: ProductListingIngestionRejectionReason,
        retryable: bool,
    },
    /// The send may have reached the queue; callers must not report it as confirmed acceptance.
    Unconfirmed,
    NotAttempted {
        reason: ProductListingIngestionNotAttemptedReason,
    },
}

/// Indexed outcome aligned to the original request position, never a compacted-list position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingIngestionItemOutcome {
    pub index: usize,
    pub command_id: ProductListingIngestionCommandId,
    pub outcome: ProductListingIngestionOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_keys_accept_only_visible_ascii_up_to_128_bytes() {
        assert!(ProductListingIngestionIdempotencyKey::new("!visible~").is_ok());
        assert!(ProductListingIngestionIdempotencyKey::new("x".repeat(128)).is_ok());
        for invalid in ["", "contains space", "\n", "é", "x".repeat(129).as_str()] {
            assert_eq!(
                Err(ProductListingIngestionIdempotencyKeyError::Invalid),
                ProductListingIngestionIdempotencyKey::new(invalid)
            );
        }
    }
}
