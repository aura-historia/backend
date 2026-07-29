use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::StructuredAddress;
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateUserCommand {
    pub user_id: UserId,
    pub email: PatchField<Email>,
    pub first_name: PatchField<FirstName>,
    pub last_name: PatchField<LastName>,
    pub language: PatchField<Language>,
    pub currency: PatchField<Currency>,
    pub measurement_unit: PatchField<MeasurementUnit>,
    pub prohibited_content_consent: PatchField<bool>,
    pub tier: PatchField<UserTier>,
    pub role: PatchField<UserRole>,
    pub stripe_customer_id: PatchField<StripeCustomerId>,
    pub structured_address: PatchField<StructuredAddress>,
}

impl UpdateUserCommand {
    pub fn is_empty(&self) -> bool {
        !self.email.is_changed()
            && !self.first_name.is_changed()
            && !self.last_name.is_changed()
            && !self.language.is_changed()
            && !self.currency.is_changed()
            && !self.measurement_unit.is_changed()
            && !self.prohibited_content_consent.is_changed()
            && !self.tier.is_changed()
            && !self.role.is_changed()
            && !self.stripe_customer_id.is_changed()
            && !self.structured_address.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserResult {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateUserError {
    #[error("user email already exists")]
    EmailConflict,
    #[error("user stripe customer already exists")]
    StripeCustomerConflict,
}

#[async_trait::async_trait]
pub trait UpdateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateUserCommand,
    ) -> Result<UpdateUserResult, UpdateUserError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_field_set() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            tier: PatchField::Set(UserTier::Pro),
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_optional_field_cleared() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            first_name: PatchField::Clear,
            ..Default::default()
        };

        assert!(!command.is_empty());
    }
}
