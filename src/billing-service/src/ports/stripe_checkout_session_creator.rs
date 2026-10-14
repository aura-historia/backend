use super::StripeBillingError;
use url::Url;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripeCheckoutSessionRequest {
    pub redirects: Option<CheckoutRedirects>,
    pub user_id: UserId,
    pub stripe_customer_id: StripeCustomerId,
    pub price_id: String,
    pub idempotency_key: Option<String>,
}

#[async_trait::async_trait]
pub trait StripeCheckoutSessionCreator: Send + Sync {
    fn redirects(&self) -> Option<CheckoutRedirects> {
        None
    }
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRedirects {
    pub success: Url,
    pub cancel: Url,
}
