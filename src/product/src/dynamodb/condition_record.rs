use crate::core::condition::Condition;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConditionRecord {
    Excellent,
    Great,
    Good,
    Fair,
    Poor,

    #[default]
    Unknown,
}

impl From<ConditionRecord> for Condition {
    fn from(record: ConditionRecord) -> Self {
        match record {
            ConditionRecord::Excellent => Condition::Excellent,
            ConditionRecord::Great => Condition::Great,
            ConditionRecord::Good => Condition::Good,
            ConditionRecord::Fair => Condition::Fair,
            ConditionRecord::Poor => Condition::Poor,
            ConditionRecord::Unknown => Condition::Unknown,
        }
    }
}

impl From<Condition> for ConditionRecord {
    fn from(value: Condition) -> Self {
        match value {
            Condition::Excellent => ConditionRecord::Excellent,
            Condition::Great => ConditionRecord::Great,
            Condition::Good => ConditionRecord::Good,
            Condition::Fair => ConditionRecord::Fair,
            Condition::Poor => ConditionRecord::Poor,
            Condition::Unknown => ConditionRecord::Unknown,
        }
    }
}
