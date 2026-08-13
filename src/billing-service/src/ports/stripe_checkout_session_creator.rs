use super::StripeBillingError;
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;
use url::Url;

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
