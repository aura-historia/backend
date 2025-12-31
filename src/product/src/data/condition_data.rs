use crate::core::condition::Condition;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConditionData {
    Excellent,
    Great,
    Good,
    Fair,
    Poor,

    #[default]
    Unknown,
}

impl From<Condition> for ConditionData {
    fn from(value: Condition) -> Self {
        match value {
            Condition::Excellent => ConditionData::Excellent,
            Condition::Great => ConditionData::Great,
            Condition::Good => ConditionData::Good,
            Condition::Fair => ConditionData::Fair,
            Condition::Poor => ConditionData::Poor,
            Condition::Unknown => ConditionData::Unknown,
        }
    }
}
