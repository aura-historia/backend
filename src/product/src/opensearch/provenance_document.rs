use crate::{core::provenance::Provenance, dynamodb::provenance_record::ProvenanceRecord};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    Default,
    Serialize,
    Deserialize,
    strum_macros::EnumCount,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvenanceDocument {
    Complete,
    Partial,
    Claimed,
    None,

    #[default]
    Unknown,
}

impl ProvenanceDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProvenanceDocument::Complete => "COMPLETE",
            ProvenanceDocument::Partial => "PARTIAL",
            ProvenanceDocument::Claimed => "CLAIMED",
            ProvenanceDocument::None => "NONE",
            ProvenanceDocument::Unknown => "UNKNOWN",
        }
    }
}

impl From<ProvenanceRecord> for ProvenanceDocument {
    fn from(record: ProvenanceRecord) -> Self {
        match record {
            ProvenanceRecord::Complete => ProvenanceDocument::Complete,
            ProvenanceRecord::Partial => ProvenanceDocument::Partial,
            ProvenanceRecord::Claimed => ProvenanceDocument::Claimed,
            ProvenanceRecord::None => ProvenanceDocument::None,
            ProvenanceRecord::Unknown => ProvenanceDocument::Unknown,
        }
    }
}

impl From<ProvenanceDocument> for Provenance {
    fn from(doc: ProvenanceDocument) -> Self {
        match doc {
            ProvenanceDocument::Complete => Provenance::Complete,
            ProvenanceDocument::Partial => Provenance::Partial,
            ProvenanceDocument::Claimed => Provenance::Claimed,
            ProvenanceDocument::None => Provenance::None,
            ProvenanceDocument::Unknown => Provenance::Unknown,
        }
    }
}

impl From<Provenance> for ProvenanceDocument {
    fn from(value: Provenance) -> Self {
        match value {
            Provenance::Complete => ProvenanceDocument::Complete,
            Provenance::Partial => ProvenanceDocument::Partial,
            Provenance::Claimed => ProvenanceDocument::Claimed,
            Provenance::None => ProvenanceDocument::None,
            Provenance::Unknown => ProvenanceDocument::Unknown,
        }
    }
}

