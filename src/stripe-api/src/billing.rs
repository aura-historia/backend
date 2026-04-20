//! Request body for both billing endpoints.
//!
//! The frontend always specifies the desired plan and billing-cycle in the
//! request body; the lambda then maps that combination to the corresponding
//! Stripe `Price` id via the four `STRIPE_<PLAN>_<CYCLE>_PRICE_ID` env-vars.

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

/// Maps `(plan, cycle)` to the matching env-var name carrying the Stripe
/// `price_…` id. The names are kept in lockstep with the CloudFormation
/// templates and are stable across all stages.
impl BillingRequest {
    pub fn price_id_env_var(&self) -> &'static str {
        match (self.plan, self.cycle) {
            (BillingPlan::Pro, BillingCycle::Monthly) => "STRIPE_PRO_MONTHLY_PRICE_ID",
            (BillingPlan::Pro, BillingCycle::Yearly) => "STRIPE_PRO_YEARLY_PRICE_ID",
            (BillingPlan::Ultimate, BillingCycle::Monthly) => "STRIPE_ULTIMATE_MONTHLY_PRICE_ID",
            (BillingPlan::Ultimate, BillingCycle::Yearly) => "STRIPE_ULTIMATE_YEARLY_PRICE_ID",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(BillingPlan::Pro, BillingCycle::Monthly, "STRIPE_PRO_MONTHLY_PRICE_ID")]
    #[case(BillingPlan::Pro, BillingCycle::Yearly, "STRIPE_PRO_YEARLY_PRICE_ID")]
    #[case(
        BillingPlan::Ultimate,
        BillingCycle::Monthly,
        "STRIPE_ULTIMATE_MONTHLY_PRICE_ID"
    )]
    #[case(
        BillingPlan::Ultimate,
        BillingCycle::Yearly,
        "STRIPE_ULTIMATE_YEARLY_PRICE_ID"
    )]
    fn should_resolve_env_var_when_plan_and_cycle_given(
        #[case] plan: BillingPlan,
        #[case] cycle: BillingCycle,
        #[case] expected: &str,
    ) {
        assert_eq!(BillingRequest { plan, cycle }.price_id_env_var(), expected);
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
