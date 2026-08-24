use billing_service::use_cases::{BillingCycle, BillingPlan, BillingSessionResult};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BillingSessionRequestData {
    #[serde(with = "crate::wire::billing_plan")]
    pub(crate) plan: BillingPlan,
    #[serde(with = "crate::wire::billing_cycle")]
    pub(crate) cycle: BillingCycle,
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
