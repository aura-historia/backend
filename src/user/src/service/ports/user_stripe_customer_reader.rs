#![allow(dead_code)]

use crate::service::use_cases::queries::find_user_by_stripe_customer_id::{
    FindUserByStripeCustomerIdRequest, UserStripeLookupView,
};

#[derive(Debug, thiserror::Error)]
pub enum UserStripeCustomerReadError {
    #[error("temporary user stripe customer lookup failure")]
    TemporarilyUnavailable,
    #[error("invalid user stripe customer read model")]
    InvalidReadModel,
    #[error("internal user stripe customer lookup failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait UserStripeCustomerReader: Send + Sync {
    async fn find_by_stripe_customer_id(
        &self,
        request: &FindUserByStripeCustomerIdRequest,
    ) -> Result<Option<UserStripeLookupView>, UserStripeCustomerReadError>;
}
