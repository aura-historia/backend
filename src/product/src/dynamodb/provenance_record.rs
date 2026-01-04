use crate::core::provenance::Provenance;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvenanceRecord {
    Complete,
    Partial,
    Claimed,
    None,

    #[default]
    Unknown,
}

impl From<ProvenanceRecord> for Provenance {
    fn from(record: ProvenanceRecord) -> Self {
        match record {
            ProvenanceRecord::Complete => Provenance::Complete,
            ProvenanceRecord::Partial => Provenance::Partial,
            ProvenanceRecord::Claimed => Provenance::Claimed,
            ProvenanceRecord::None => Provenance::None,
            ProvenanceRecord::Unknown => Provenance::Unknown,
        }
    }
}

impl From<Provenance> for ProvenanceRecord {
    fn from(value: Provenance) -> Self {
        match value {
            Provenance::Complete => ProvenanceRecord::Complete,
            Provenance::Partial => ProvenanceRecord::Partial,
            Provenance::Claimed => ProvenanceRecord::Claimed,
            Provenance::None => ProvenanceRecord::None,
            Provenance::Unknown => ProvenanceRecord::Unknown,
        }
    }
}
