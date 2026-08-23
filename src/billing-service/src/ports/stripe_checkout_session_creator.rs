use super::StripeBillingError;
use url::Url;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripeCheckoutSessionRequest {
    pub user_id: UserId,
    pub stripe_customer_id: StripeCustomerId,
    pub price_id: String,
    pub idempotency_key: Option<String>,
}

#[async_trait::async_trait]
pub trait StripeCheckoutSessionCreator: Send + Sync {
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError>;
}
