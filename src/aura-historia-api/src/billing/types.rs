use billing_service::use_cases::{BillingCycle, BillingPlan, BillingSessionResult};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BillingPlanData {
    Pro,
    Ultimate,
}

impl From<BillingPlanData> for BillingPlan {
    fn from(value: BillingPlanData) -> Self {
        match value {
            BillingPlanData::Pro => Self::Pro,
            BillingPlanData::Ultimate => Self::Ultimate,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BillingCycleData {
    Monthly,
    Yearly,
}

impl From<BillingCycleData> for BillingCycle {
    fn from(value: BillingCycleData) -> Self {
        match value {
            BillingCycleData::Monthly => Self::Monthly,
            BillingCycleData::Yearly => Self::Yearly,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BillingSessionRequestData {
    pub(crate) plan: BillingPlanData,
    pub(crate) cycle: BillingCycleData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BillingSessionData {
    pub(crate) url: Url,
}

impl From<BillingSessionResult> for BillingSessionData {
    fn from(value: BillingSessionResult) -> Self {
        Self { url: value.url }
    }
}
