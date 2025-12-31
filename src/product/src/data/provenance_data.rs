use serde::{Deserialize, Serialize};
use crate::core::provenance::Provenance;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvenanceData {
    Complete,
    Partial,
    Claimed,
    None,

    #[default]
    Unknown,
}

impl From<Provenance> for ProvenanceData {
    fn from(value: Provenance) -> Self {
        match value {
            Provenance::Complete => ProvenanceData::Complete,
            Provenance::Partial => ProvenanceData::Partial,
            Provenance::Claimed => ProvenanceData::Claimed,
            Provenance::None => ProvenanceData::None,
            Provenance::Unknown => ProvenanceData::Unknown,
        }
    }
}
