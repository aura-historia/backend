use super::StripeBillingError;
use url::Url;
use user_core::stripe_customer_id::StripeCustomerId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStripePortalSessionRequest {
    pub stripe_customer_id: StripeCustomerId,
    pub idempotency_key: Option<String>,
}

#[async_trait::async_trait]
pub trait StripePortalSessionCreator: Send + Sync {
    async fn create_portal_session(
        &self,
        request: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError>;
}
