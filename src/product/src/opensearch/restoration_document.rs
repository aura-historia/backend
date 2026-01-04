use crate::{core::restoration::Restoration, dynamodb::restoration_record::RestorationRecord};
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
pub enum RestorationDocument {
    None,
    Minor,
    Major,

    #[default]
    Unknown,
}

impl RestorationDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            RestorationDocument::None => "NONE",
            RestorationDocument::Minor => "MINOR",
            RestorationDocument::Major => "MAJOR",
            RestorationDocument::Unknown => "UNKNOWN",
        }
    }
}

impl From<RestorationRecord> for RestorationDocument {
    fn from(record: RestorationRecord) -> Self {
        match record {
            RestorationRecord::None => RestorationDocument::None,
            RestorationRecord::Minor => RestorationDocument::Minor,
            RestorationRecord::Major => RestorationDocument::Major,
            RestorationRecord::Unknown => RestorationDocument::Unknown,
        }
    }
}

impl From<RestorationDocument> for Restoration {
    fn from(doc: RestorationDocument) -> Self {
        match doc {
            RestorationDocument::None => Restoration::None,
            RestorationDocument::Minor => Restoration::Minor,
            RestorationDocument::Major => Restoration::Major,
            RestorationDocument::Unknown => Restoration::Unknown,
        }
    }
}

impl From<Restoration> for RestorationDocument {
    fn from(value: Restoration) -> Self {
        match value {
            Restoration::None => RestorationDocument::None,
            Restoration::Minor => RestorationDocument::Minor,
            Restoration::Major => RestorationDocument::Major,
            Restoration::Unknown => RestorationDocument::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RestorationDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(RestorationDocument::None, "\"NONE\"")]
    #[case(RestorationDocument::Minor, "\"MINOR\"")]
    #[case(RestorationDocument::Major, "\"MAJOR\"")]
    #[case(RestorationDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_restoration_document_in_screaming_snake_case(
        #[case] restoration: RestorationDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&restoration).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"NONE\"", RestorationDocument::None)]
    #[case("\"MINOR\"", RestorationDocument::Minor)]
    #[case("\"MAJOR\"", RestorationDocument::Major)]
    #[case("\"UNKNOWN\"", RestorationDocument::Unknown)]
    fn should_deserialize_restoration_document_in_screaming_snake_case(
        #[case] restoration: &str,
        #[case] expected: RestorationDocument,
    ) {
        let actual = serde_json::from_str::<RestorationDocument>(restoration).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(RestorationDocument::None)]
    #[case(RestorationDocument::Minor)]
    #[case(RestorationDocument::Major)]
    #[case(RestorationDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] restoration: RestorationDocument) {
        let serialized = serde_json::to_string::<RestorationDocument>(&restoration)
            .unwrap()
            .replace("\"", "");
        let as_str = restoration.as_str();
        assert_eq!(serialized, as_str);
    }
}
