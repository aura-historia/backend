use crate::core::{role::UserRole, tier::UserTier};
use common::operation_context::OperationContext;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct FindUserByStripeCustomerIdRequest {
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserStripeLookupView {
    pub user_id: UserId,
    pub email: Email,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, thiserror::Error)]
pub enum FindUserByStripeCustomerIdError {}

#[async_trait::async_trait]
pub trait FindUserByStripeCustomerIdUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: FindUserByStripeCustomerIdRequest,
    ) -> Result<UserStripeLookupView, FindUserByStripeCustomerIdError>;
}
