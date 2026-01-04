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

#[cfg(test)]
mod tests {
    use super::ProvenanceDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(ProvenanceDocument::Complete, "\"COMPLETE\"")]
    #[case(ProvenanceDocument::Partial, "\"PARTIAL\"")]
    #[case(ProvenanceDocument::Claimed, "\"CLAIMED\"")]
    #[case(ProvenanceDocument::None, "\"NONE\"")]
    #[case(ProvenanceDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_provenance_document_in_screaming_snake_case(
        #[case] provenance: ProvenanceDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&provenance).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"COMPLETE\"", ProvenanceDocument::Complete)]
    #[case("\"PARTIAL\"", ProvenanceDocument::Partial)]
    #[case("\"CLAIMED\"", ProvenanceDocument::Claimed)]
    #[case("\"NONE\"", ProvenanceDocument::None)]
    #[case("\"UNKNOWN\"", ProvenanceDocument::Unknown)]
    fn should_deserialize_provenance_document_in_screaming_snake_case(
        #[case] provenance: &str,
        #[case] expected: ProvenanceDocument,
    ) {
        let actual = serde_json::from_str::<ProvenanceDocument>(provenance).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(ProvenanceDocument::Complete)]
    #[case(ProvenanceDocument::Partial)]
    #[case(ProvenanceDocument::Claimed)]
    #[case(ProvenanceDocument::None)]
    #[case(ProvenanceDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] provenance: ProvenanceDocument) {
        let serialized = serde_json::to_string::<ProvenanceDocument>(&provenance)
            .unwrap()
            .replace("\"", "");
        let as_str = provenance.as_str();
        assert_eq!(serialized, as_str);
    }
}
