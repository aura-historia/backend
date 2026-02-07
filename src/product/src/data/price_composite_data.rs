use common::price::data::PriceData;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceCompositeData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub estimate_min: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub offer: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub estimate_max: Option<PriceData>,
}
