use super::StripeBillingError;
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;
use serde_email::Email;

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
