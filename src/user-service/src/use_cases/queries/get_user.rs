use common::operation_context::OperationContext;
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::{GeoAddress, StructuredAddress};
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq)]
pub enum GetUserRequest {
    ById(UserId),
    ByEmail(Email),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserDetailsView {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub prohibited_content_consent: bool,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetUserError {}

#[async_trait::async_trait]
pub trait GetUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetUserRequest,
    ) -> Result<UserDetailsView, GetUserError>;
}
