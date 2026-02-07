use crate::core::origin_year::OriginYear;
use common::year::Year;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginYearData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max: Option<Year>,
}

impl From<OriginYear> for OriginYearData {
    fn from(origin_year: OriginYear) -> Self {
        Self {
            min: origin_year.min(),
            year: origin_year.exact(),
            max: origin_year.max(),
        }
    }
}
