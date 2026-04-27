//! Request body for both billing endpoints.
//!
//! The frontend always specifies the desired plan and billing-cycle in the
//! request body; the lambda then maps that combination to the corresponding
//! Stripe price lookup key and resolves the live `Price` id at runtime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BillingPlan {
    Pro,
    Ultimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BillingCycle {
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingRequest {
    pub plan: BillingPlan,
    pub cycle: BillingCycle,
}

/// Maps `(plan, cycle)` to the Stripe price lookup key.
///
/// The lookup key format is `[PLAN]_[BILLING]` (all lowercase), matching the
/// schema pre-configured in the Stripe dashboard.
impl BillingRequest {
    pub fn lookup_key(&self) -> &'static str {
        match (self.plan, self.cycle) {
            (BillingPlan::Pro, BillingCycle::Monthly) => "pro_monthly",
            (BillingPlan::Pro, BillingCycle::Yearly) => "pro_yearly",
            (BillingPlan::Ultimate, BillingCycle::Monthly) => "ultimate_monthly",
            (BillingPlan::Ultimate, BillingCycle::Yearly) => "ultimate_yearly",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(BillingPlan::Pro, BillingCycle::Monthly, "pro_monthly")]
    #[case(BillingPlan::Pro, BillingCycle::Yearly, "pro_yearly")]
    #[case(BillingPlan::Ultimate, BillingCycle::Monthly, "ultimate_monthly")]
    #[case(BillingPlan::Ultimate, BillingCycle::Yearly, "ultimate_yearly")]
    fn should_resolve_lookup_key_when_plan_and_cycle_given(
        #[case] plan: BillingPlan,
        #[case] cycle: BillingCycle,
        #[case] expected: &str,
    ) {
        assert_eq!(BillingRequest { plan, cycle }.lookup_key(), expected);
    }

    #[rstest]
    #[case(
        r#"{"plan":"PRO","cycle":"MONTHLY"}"#,
        BillingPlan::Pro,
        BillingCycle::Monthly
    )]
    #[case(
        r#"{"plan":"PRO","cycle":"YEARLY"}"#,
        BillingPlan::Pro,
        BillingCycle::Yearly
    )]
    #[case(
        r#"{"plan":"ULTIMATE","cycle":"MONTHLY"}"#,
        BillingPlan::Ultimate,
        BillingCycle::Monthly
    )]
    #[case(
        r#"{"plan":"ULTIMATE","cycle":"YEARLY"}"#,
        BillingPlan::Ultimate,
        BillingCycle::Yearly
    )]
    fn should_deserialize_billing_request_when_valid_json(
        #[case] json: &str,
        #[case] plan: BillingPlan,
        #[case] cycle: BillingCycle,
    ) {
        let actual: BillingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(actual, BillingRequest { plan, cycle });
    }

    #[test]
    fn should_fail_deserialize_when_unknown_plan() {
        let result: Result<BillingRequest, _> =
            serde_json::from_str(r#"{"plan":"ENTERPRISE","cycle":"MONTHLY"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn should_fail_deserialize_when_unknown_cycle() {
        let result: Result<BillingRequest, _> =
            serde_json::from_str(r#"{"plan":"PRO","cycle":"DAILY"}"#);
        assert!(result.is_err());
    }
}
