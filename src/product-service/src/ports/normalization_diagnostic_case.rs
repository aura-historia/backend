//! Retained corpus of raw revisions whose worker normalization produced unresolved or failed
//! outcomes.
//!
//! Cases are diagnostic evidence for heuristic improvement and replay. They never drive canonical
//! writes, head advancement or redelivery. Crawler-side extraction checks are not recorded here.

use application::error::BoxError;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_raw_id::ProductListingRawRevisionId;
use product_listing_normalization::{NormalizationIssue, ProductListingNormalizationInput};
use sha2::{Digest, Sha256};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

pub const NORMALIZATION_DIAGNOSTIC_INPUT_SHA256_BYTES: usize = 32;
pub const MAX_NORMALIZATION_DIAGNOSTIC_BUILD_BYTES: usize = 64;

/// Versioned evaluator entry point. The version names the evidence contract, not the heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum NormalizationDiagnosticEntryPoint {
    /// Worker evaluation of one immutable raw revision.
    RawValuesNormalizationV1,
}

impl NormalizationDiagnosticEntryPoint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RawValuesNormalizationV1 => "RAW_VALUES_NORMALIZATION_V1",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|entry_point| entry_point.as_str() == value)
    }
}

/// Recorded outcome of the evaluation that produced the case. Evaluation outcomes (`EVALUATED_*`)
/// are distinct from committed application outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum NormalizationDiagnosticOutcome {
    Applied,
    NoChange,
    Rejected,
    SystemFailure,
    EvaluatedResolved,
    EvaluatedInvalid,
}

impl NormalizationDiagnosticOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "APPLIED",
            Self::NoChange => "NO_CHANGE",
            Self::Rejected => "REJECTED",
            Self::SystemFailure => "SYSTEM_FAILURE",
            Self::EvaluatedResolved => "EVALUATED_RESOLVED",
            Self::EvaluatedInvalid => "EVALUATED_INVALID",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|outcome| outcome.as_str() == value)
    }
}

/// Stable case identity derived from the entry point and exact input hash. Implementation
/// version is provenance and never part of the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NormalizationDiagnosticCaseId(uuid::Uuid);

impl NormalizationDiagnosticCaseId {
    pub fn derive(
        entry_point: NormalizationDiagnosticEntryPoint,
        input_sha256: &[u8; NORMALIZATION_DIAGNOSTIC_INPUT_SHA256_BYTES],
    ) -> Self {
        let mut digest = Sha256::new();
        digest.update(entry_point.as_str().as_bytes());
        digest.update([0]);
        digest.update(input_sha256);
        let digest = digest.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(uuid::Builder::from_custom_bytes(bytes).into_uuid())
    }

    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl std::fmt::Display for NormalizationDiagnosticCaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Diagnostic provenance. `build` is the deploying Git SHA, or `unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationDiagnosticProvenance {
    pub diagnostics_version: u16,
    pub evaluator_version: u16,
    pub build: String,
}

/// One observation to record. Recording the same identity again updates the latest outcome,
/// issues and provenance and increments the occurrence count.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizationDiagnosticCase {
    pub entry_point: NormalizationDiagnosticEntryPoint,
    pub input_sha256: [u8; NORMALIZATION_DIAGNOSTIC_INPUT_SHA256_BYTES],
    pub listing_source_id: ListingSourceId,
    /// The immutable raw revision holding the complete normalization input.
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub outcome: NormalizationDiagnosticOutcome,
    pub issues: Vec<NormalizationIssue>,
    pub provenance: NormalizationDiagnosticProvenance,
}

impl NormalizationDiagnosticCase {
    pub fn case_id(&self) -> NormalizationDiagnosticCaseId {
        NormalizationDiagnosticCaseId::derive(self.entry_point, &self.input_sha256)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizationDiagnosticCasePortError {
    #[error("normalization diagnostic case persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("normalization diagnostic case persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Records cases inside the caller's transaction so terminal cases commit with completion.
#[async_trait]
pub trait NormalizationDiagnosticCaseWriter: Send {
    async fn record(
        &mut self,
        case: NormalizationDiagnosticCase,
    ) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError>;
}

pub trait NormalizationDiagnosticCaseWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl NormalizationDiagnosticCaseWriter + 'tx;
}

/// Records one case in its own short transaction, for historical seeding.
#[async_trait]
pub trait NormalizationDiagnosticCaseRecorder: Send + Sync {
    async fn record(
        &self,
        case: NormalizationDiagnosticCase,
    ) -> Result<NormalizationDiagnosticCaseId, NormalizationDiagnosticCasePortError>;
}

/// Replay selection. No filter selects the whole corpus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizationDiagnosticCaseFilter {
    pub entry_point: Option<NormalizationDiagnosticEntryPoint>,
    pub listing_source_id: Option<ListingSourceId>,
    pub case_ids: Vec<NormalizationDiagnosticCaseId>,
    /// Persisted field code from a recorded issue, for example `PRICE`.
    pub field: Option<String>,
    /// Persisted reason code from a recorded issue, for example `PRICE_UNKNOWN_CURRENCY`.
    pub reason: Option<String>,
}

/// One keyset page of a corpus snapshot bounded at run start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationDiagnosticCasePageRequest {
    pub after_generation: i64,
    pub through_generation: i64,
    pub limit: u32,
    pub filter: NormalizationDiagnosticCaseFilter,
}

/// Evidence resolved for replay. Unavailable evidence is explicit, never silently skipped.
#[derive(Debug, Clone, PartialEq)]
pub enum StoredNormalizationDiagnosticEvidence {
    RawRevision {
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        input: Box<ProductListingNormalizationInput>,
    },
    Unavailable {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredNormalizationDiagnosticCase {
    pub case_id: NormalizationDiagnosticCaseId,
    pub generation: i64,
    pub entry_point: Option<NormalizationDiagnosticEntryPoint>,
    pub entry_point_code: String,
    pub input_sha256: Vec<u8>,
    pub listing_source_id: uuid::Uuid,
    pub evidence: StoredNormalizationDiagnosticEvidence,
    pub recorded_outcome: String,
    /// Recorded issues as persisted. Older vocabularies remain readable as JSON.
    pub recorded_issues: serde_json::Value,
    pub diagnostics_version: i16,
    pub evaluator_version: i16,
    pub build: String,
    pub occurrences: i64,
}

#[async_trait]
pub trait NormalizationDiagnosticCaseReader: Send + Sync {
    /// Highest generation at run start. Cases recorded later belong to the next run.
    async fn snapshot_bound(&self) -> Result<Option<i64>, NormalizationDiagnosticCasePortError>;

    async fn page(
        &self,
        request: NormalizationDiagnosticCasePageRequest,
    ) -> Result<Vec<StoredNormalizationDiagnosticCase>, NormalizationDiagnosticCasePortError>;
}

/// One retained raw revision for historical corpus seeding.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedProductListingRawRevision {
    pub generation: i64,
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub listing_source_id: ListingSourceId,
    pub input: ProductListingNormalizationInput,
}

/// Retained raw revision whose stored input no longer satisfies the input contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableProductListingRawRevision {
    pub generation: i64,
    pub product_listing_raw_revision_id: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RetainedProductListingRawRevisionRead {
    Readable(Box<RetainedProductListingRawRevision>),
    Unreadable(UnreadableProductListingRawRevision),
}

/// Pages retained UPSERT raw revisions by generation, bounded at seeding start.
#[async_trait]
pub trait RetainedProductListingRawRevisionReader: Send + Sync {
    async fn snapshot_bound(&self) -> Result<Option<i64>, NormalizationDiagnosticCasePortError>;

    async fn page(
        &self,
        after_generation: i64,
        through_generation: i64,
        limit: u32,
    ) -> Result<Vec<RetainedProductListingRawRevisionRead>, NormalizationDiagnosticCasePortError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_identity_depends_only_on_entry_point_and_input_hash() {
        let hash = [7_u8; NORMALIZATION_DIAGNOSTIC_INPUT_SHA256_BYTES];
        let raw = NormalizationDiagnosticCaseId::derive(
            NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1,
            &hash,
        );
        assert_eq!(
            raw,
            NormalizationDiagnosticCaseId::derive(
                NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1,
                &hash,
            )
        );
        assert_ne!(
            raw,
            NormalizationDiagnosticCaseId::derive(
                NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1,
                &[8; NORMALIZATION_DIAGNOSTIC_INPUT_SHA256_BYTES],
            )
        );
    }

    #[test]
    fn persisted_codes_round_trip() {
        for entry_point in NormalizationDiagnosticEntryPoint::iter() {
            assert_eq!(
                Some(entry_point),
                NormalizationDiagnosticEntryPoint::from_code(entry_point.as_str())
            );
        }
        for outcome in NormalizationDiagnosticOutcome::iter() {
            assert_eq!(
                Some(outcome),
                NormalizationDiagnosticOutcome::from_code(outcome.as_str())
            );
        }
    }
}
