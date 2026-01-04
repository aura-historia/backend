use crate::{core::condition::Condition, dynamodb::condition_record::ConditionRecord};
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
pub enum ConditionDocument {
    Excellent,
    Great,
    Good,
    Fair,
    Poor,

    #[default]
    Unknown,
}

impl ConditionDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConditionDocument::Excellent => "EXCELLENT",
            ConditionDocument::Great => "GREAT",
            ConditionDocument::Good => "GOOD",
            ConditionDocument::Fair => "FAIR",
            ConditionDocument::Poor => "POOR",
            ConditionDocument::Unknown => "UNKNOWN",
        }
    }
}

impl From<ConditionRecord> for ConditionDocument {
    fn from(record: ConditionRecord) -> Self {
        match record {
            ConditionRecord::Excellent => ConditionDocument::Excellent,
            ConditionRecord::Great => ConditionDocument::Great,
            ConditionRecord::Good => ConditionDocument::Good,
            ConditionRecord::Fair => ConditionDocument::Fair,
            ConditionRecord::Poor => ConditionDocument::Poor,
            ConditionRecord::Unknown => ConditionDocument::Unknown,
        }
    }
}

impl From<ConditionDocument> for Condition {
    fn from(doc: ConditionDocument) -> Self {
        match doc {
            ConditionDocument::Excellent => Condition::Excellent,
            ConditionDocument::Great => Condition::Great,
            ConditionDocument::Good => Condition::Good,
            ConditionDocument::Fair => Condition::Fair,
            ConditionDocument::Poor => Condition::Poor,
            ConditionDocument::Unknown => Condition::Unknown,
        }
    }
}

impl From<Condition> for ConditionDocument {
    fn from(value: Condition) -> Self {
        match value {
            Condition::Excellent => ConditionDocument::Excellent,
            Condition::Great => ConditionDocument::Great,
            Condition::Good => ConditionDocument::Good,
            Condition::Fair => ConditionDocument::Fair,
            Condition::Poor => ConditionDocument::Poor,
            Condition::Unknown => ConditionDocument::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConditionDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(ConditionDocument::Excellent, "\"EXCELLENT\"")]
    #[case(ConditionDocument::Great, "\"GREAT\"")]
    #[case(ConditionDocument::Good, "\"GOOD\"")]
    #[case(ConditionDocument::Fair, "\"FAIR\"")]
    #[case(ConditionDocument::Poor, "\"POOR\"")]
    #[case(ConditionDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_condition_document_in_screaming_snake_case(
        #[case] condition: ConditionDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&condition).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"EXCELLENT\"", ConditionDocument::Excellent)]
    #[case("\"GREAT\"", ConditionDocument::Great)]
    #[case("\"GOOD\"", ConditionDocument::Good)]
    #[case("\"FAIR\"", ConditionDocument::Fair)]
    #[case("\"POOR\"", ConditionDocument::Poor)]
    #[case("\"UNKNOWN\"", ConditionDocument::Unknown)]
    fn should_deserialize_condition_document_in_screaming_snake_case(
        #[case] condition: &str,
        #[case] expected: ConditionDocument,
    ) {
        let actual = serde_json::from_str::<ConditionDocument>(condition).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(ConditionDocument::Excellent)]
    #[case(ConditionDocument::Great)]
    #[case(ConditionDocument::Good)]
    #[case(ConditionDocument::Fair)]
    #[case(ConditionDocument::Poor)]
    #[case(ConditionDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] condition: ConditionDocument) {
        let serialized = serde_json::to_string::<ConditionDocument>(&condition)
            .unwrap()
            .replace("\"", "");
        let as_str = condition.as_str();
        assert_eq!(serialized, as_str);
    }
}
