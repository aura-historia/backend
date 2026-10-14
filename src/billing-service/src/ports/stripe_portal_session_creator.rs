use super::StripeBillingError;
use url::Url;
use user_core::stripe_customer_id::StripeCustomerId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripePortalSessionRequest {
    pub return_url: Option<Url>,
    pub stripe_customer_id: StripeCustomerId,
    pub idempotency_key: Option<String>,
}

#[async_trait::async_trait]
pub trait StripePortalSessionCreator: Send + Sync {
    fn return_url(&self) -> Option<Url> {
        None
    }
    async fn create_portal_session(
        &self,
        request: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError>;
}
