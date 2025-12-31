use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Year(i32);

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Serialize, Deserialize)]
pub struct YearRange {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max: Option<Year>,
}

impl From<i32> for Year {
    fn from(value: i32) -> Self {
        Year(value)
    }
}
