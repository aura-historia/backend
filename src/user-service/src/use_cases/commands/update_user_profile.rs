use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserDetailsView, UserRepository,
    UserRepositoryError, UserRepositoryFactory,
};
use crate::use_cases::authorization::{RequireAdminActorError, require_admin_actor};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::Language;
use money::Currency;
use serde_email::Email;
use user_core::measurement_unit::MeasurementUnit;
use user_core::user::{RehydrateUserError, User, UserPreferences, UserProfile};
use user_core::user_id::UserId;
use user_core::{first_name::FirstName, last_name::LastName};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateUserProfileCommand {
    pub user_id: UserId,
    pub email: PatchField<Email>,
    pub first_name: PatchField<FirstName>,
    pub last_name: PatchField<LastName>,
    pub language: PatchField<Language>,
    pub currency: PatchField<Currency>,
    pub measurement_unit: PatchField<MeasurementUnit>,
    pub show_unassessed_or_sensitive_content: PatchField<bool>,
}

impl UpdateUserProfileCommand {
    pub fn is_empty(&self) -> bool {
        !self.email.is_changed()
            && !self.first_name.is_changed()
            && !self.last_name.is_changed()
            && !self.language.is_changed()
            && !self.currency.is_changed()
            && !self.measurement_unit.is_changed()
            && !self.show_unassessed_or_sensitive_content.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserProfileResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateUserProfileError {
    #[error("authenticated actor required to update user profile")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email already exists")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("user email is required")]
    EmailRequired,
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
    #[error("failed to begin update user profile transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update user profile transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateUserProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateUserProfileCommand,
    ) -> Result<UpdateUserProfileResult, UpdateUserProfileError>;
}

pub struct UpdateUserProfileHandler<U, R, A> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
    admin_only: bool,
}

impl<U, R, A> UpdateUserProfileHandler<U, R, A> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
            admin_only: false,
        }
    }

    pub fn new_admin_only(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
            admin_only: true,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> UpdateUserProfileUseCase for UpdateUserProfileHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_user_profile",
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
        command: UpdateUserProfileCommand,
    ) -> Result<UpdateUserProfileResult, UpdateUserProfileError> {
        context
            .require()
            .credential_capability(CredentialCapability::UsersWrite)
            .authorize::<UpdateUserProfileError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateUserProfileError::BeginTransactionFailed)?;
        authorize_user_profile_write(
            context,
            command.user_id,
            self.admin_only,
            &mut tx,
            &self.admin_reader,
        )
        .await?;
        let mut users = self.users.in_transaction(&mut tx);
        let domain_primitives::versioned::Versioned {
            value: mut user,
            version,
        } = users
            .find_by_id(command.user_id)
            .await?
            .ok_or(UpdateUserProfileError::UserNotFound)?;

        let outcome = apply_update(&mut user, command)?;
        if outcome.changed() {
            user = users.update(&user, version).await?.value;
        }
        drop(users);

        tx.commit()
            .await
            .map_err(|_| UpdateUserProfileError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.profile_updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(UpdateUserProfileResult {
            view: UserDetailsView::from(&user),
        })
    }
}

fn apply_update(
    user: &mut User,
    command: UpdateUserProfileCommand,
) -> Result<ChangeOutcome, UpdateUserProfileError> {
    let mut outcome = ChangeOutcome::Unchanged;

    outcome = outcome.combine(match command.email {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_email(value),
        PatchField::Clear => return Err(UpdateUserProfileError::EmailRequired),
    });

    let mut profile = user.profile().clone();
    let profile_before = profile.clone();
    apply_optional_patch(&mut profile.first_name, command.first_name);
    apply_optional_patch(&mut profile.last_name, command.last_name);

    if profile != profile_before {
        outcome = outcome.combine(user.replace_profile(profile)?);
    }

    let mut preferences = user.preferences().clone();
    let preferences_before = preferences.clone();
    apply_optional_patch(&mut preferences.language, command.language);
    apply_optional_patch(&mut preferences.currency, command.currency);
    apply_optional_patch(&mut preferences.measurement_unit, command.measurement_unit);
    match command.show_unassessed_or_sensitive_content {
        PatchField::Unchanged => {}
        PatchField::Set(value) => preferences.show_unassessed_or_sensitive_content = value,
        PatchField::Clear => preferences.show_unassessed_or_sensitive_content = false,
    }
    if preferences != preferences_before {
        outcome = outcome.combine(user.replace_preferences(preferences));
    }

    Ok(outcome)
}

fn apply_optional_patch<T>(target: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *target = Some(value),
        PatchField::Clear => *target = None,
    }
}

async fn authorize_user_profile_write<Tx, A>(
    context: &OperationContext,
    user_id: UserId,
    admin_only: bool,
    tx: &mut Tx,
    admin_reader: &A,
) -> Result<(), UpdateUserProfileError>
where
    Tx: Transaction,
    A: UserAdminReaderFactory<Tx>,
{
    if admin_only {
        let mut reader = admin_reader.in_transaction(tx);
        return require_admin_actor(context, &mut reader)
            .await
            .map_err(UpdateUserProfileError::from);
    }

    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(actor_id)
        | Principal::DelegatedUser {
            user_id: actor_id, ..
        } if *actor_id == user_id => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            let mut reader = admin_reader.in_transaction(tx);
            require_admin_actor(context, &mut reader)
                .await
                .map_err(UpdateUserProfileError::from)
        }
        Principal::Anonymous => Err(UpdateUserProfileError::AuthenticatedActorRequired),
    }
}

impl From<&User> for UserDetailsView {
    fn from(user: &User) -> Self {
        let profile: &UserProfile = user.profile();
        let preferences: &UserPreferences = user.preferences();
        Self {
            user_id: user.id(),
            email: user.email().clone(),
            first_name: profile.first_name.clone(),
            last_name: profile.last_name.clone(),
            language: preferences.language,
            currency: preferences.currency,
            measurement_unit: preferences.measurement_unit,
            show_unassessed_or_sensitive_content: preferences.show_unassessed_or_sensitive_content,
            tier: user.account().tier,
            role: user.account().role,
            stripe_customer_id: user.account().stripe_customer_id.clone(),
        }
    }
}

impl From<OperationAuthorizationError> for UpdateUserProfileError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<RequireAdminActorError> for UpdateUserProfileError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for UpdateUserProfileError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<UserRepositoryError> for UpdateUserProfileError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source } => Self::EmailConflict { source },
            UserRepositoryError::StripeCustomerConflict { source } => Self::Internal { source },
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

impl From<RehydrateUserError> for UpdateUserProfileError {
    fn from(error: RehydrateUserError) -> Self {
        Self::InvalidUserState {
            source: box_error(error),
        }
    }
}
