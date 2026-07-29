use crate::ports::{UserRepository, UserRepositoryError, UserRepositoryFactory};
use common::change_outcome::ChangeOutcome;
use common::error::boxed::{BoxError, box_error};
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::transaction::{Transaction, UnitOfWork};
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::StructuredAddress;
use serde_email::Email;
use user_core::user::{RehydrateUserError, User};
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
    #[error("authenticated actor required to update user")]
    AuthenticatedActorRequired,
    #[error("user not found")]
    UserNotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email already exists")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("user stripe customer already exists")]
    StripeCustomerConflict {
        #[source]
        source: BoxError,
    },
    #[error("user email is required")]
    EmailRequired,
    #[error("user tier is required")]
    TierRequired,
    #[error("user role is required")]
    RoleRequired,
    #[error("invalid user state")]
    InvalidUserState {
        #[source]
        source: BoxError,
    },
    #[error("temporary user persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal user persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateUserCommand,
    ) -> Result<UpdateUserResult, UpdateUserError>;
}

pub struct UpdateUserHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> UpdateUserHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> UpdateUserUseCase for UpdateUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_user",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateUserCommand,
    ) -> Result<UpdateUserResult, UpdateUserError> {
        context
            .principal
            .require_authenticated()
            .map_err(|_| UpdateUserError::AuthenticatedActorRequired)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateUserError::BeginTransactionFailed)?;
        let common::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(UpdateUserError::UserNotFound)?;

        let outcome = apply_update(&mut user, command)?;
        if outcome.changed() {
            self.users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| UpdateUserError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(UpdateUserResult::from(&user))
    }
}

impl From<&User> for UpdateUserResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            email: user.email().clone(),
        }
    }
}

fn apply_update(
    user: &mut User,
    command: UpdateUserCommand,
) -> Result<ChangeOutcome, UpdateUserError> {
    let mut outcome = ChangeOutcome::Unchanged;

    outcome = outcome.combine(match command.email {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_email(value),
        PatchField::Clear => return Err(UpdateUserError::EmailRequired),
    });

    let mut profile = user.profile().clone();
    let profile_before = profile.clone();
    apply_optional_patch(&mut profile.first_name, command.first_name);
    apply_optional_patch(&mut profile.last_name, command.last_name);
    match command.structured_address {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            profile.structured_address = Some(value);
            profile.geo_address = None;
        }
        PatchField::Clear => {
            profile.structured_address = None;
            profile.geo_address = None;
        }
    }
    if profile != profile_before {
        outcome = outcome.combine(user.replace_profile(profile)?);
    }

    let mut preferences = user.preferences().clone();
    let preferences_before = preferences.clone();
    apply_optional_patch(&mut preferences.language, command.language);
    apply_optional_patch(&mut preferences.currency, command.currency);
    apply_optional_patch(&mut preferences.measurement_unit, command.measurement_unit);
    match command.prohibited_content_consent {
        PatchField::Unchanged => {}
        PatchField::Set(value) => preferences.prohibited_content_consent = value,
        PatchField::Clear => preferences.prohibited_content_consent = false,
    }
    if preferences != preferences_before {
        outcome = outcome.combine(user.replace_preferences(preferences));
    }

    outcome = outcome.combine(match command.tier {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_tier(value),
        PatchField::Clear => return Err(UpdateUserError::TierRequired),
    });
    outcome = outcome.combine(match command.role {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_role(value),
        PatchField::Clear => return Err(UpdateUserError::RoleRequired),
    });
    outcome = outcome.combine(match command.stripe_customer_id {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_stripe_customer_id(Some(value)),
        PatchField::Clear => user.change_stripe_customer_id(None),
    });

    Ok(outcome)
}

fn apply_optional_patch<T>(target: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *target = Some(value),
        PatchField::Clear => *target = None,
    }
}

impl From<RehydrateUserError> for UpdateUserError {
    fn from(error: RehydrateUserError) -> Self {
        Self::InvalidUserState {
            source: box_error(error),
        }
    }
}

impl From<UserRepositoryError> for UpdateUserError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source } => Self::EmailConflict { source },
            UserRepositoryError::StripeCustomerConflict { source } => {
                Self::StripeCustomerConflict { source }
            }
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            UserRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
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
