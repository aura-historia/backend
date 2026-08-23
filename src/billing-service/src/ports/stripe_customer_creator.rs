use super::StripeBillingError;
use serde_email::Email;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripeCustomerRequest {
    pub user_id: UserId,
    pub email: Email,
    pub name: Option<String>,
    pub idempotency_key: Option<String>,
}

#[async_trait::async_trait]
pub trait StripeCustomerCreator: Send + Sync {
    async fn create_customer(
        &self,
        request: CreateStripeCustomerRequest,
    ) -> Result<StripeCustomerId, StripeBillingError>;
}
