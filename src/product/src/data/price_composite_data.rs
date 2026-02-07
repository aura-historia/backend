use common::price::data::PriceData;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub offer: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub estimate: Option<PriceEstimateData>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceEstimateData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max: Option<PriceData>,
}
