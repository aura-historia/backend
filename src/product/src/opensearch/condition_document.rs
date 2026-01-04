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
