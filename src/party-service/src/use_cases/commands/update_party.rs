use crate::ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory};
use crate::use_cases::queries::get_party::PartyDetailsView;
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use party_core::{party::PartyContact, party_id::PartyId, party_name::PartyName};
use serde_email::Email;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdatePartyCommand {
    pub party_id: PartyId,
    pub name: PatchField<PartyName>,
    pub phone: PatchField<String>,
    pub email: PatchField<Email>,
}

impl UpdatePartyCommand {
    pub fn is_empty(&self) -> bool {
        !self.name.is_changed() && !self.phone.is_changed() && !self.email.is_changed()
    }
}

pub type UpdatePartyResult = PartyDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum UpdatePartyError {
    #[error("authenticated actor required to update party")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party not found")]
    NotFound,
    #[error("concurrent party update")]
    ConcurrencyConflict,
    #[error("party slug already exists")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary party persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted party state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal party persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update party transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update party transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdatePartyUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyCommand,
    ) -> Result<UpdatePartyResult, UpdatePartyError>;
}

pub struct UpdatePartyHandler<U, R, A> {
    unit_of_work: U,
    parties: R,
    check_user_admin: A,
}

impl<U, R, A> UpdatePartyHandler<U, R, A> {
    pub fn new(unit_of_work: U, parties: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> UpdatePartyUseCase for UpdatePartyHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "update_party",
        skip_all,
        fields(
            party_id = %command.party_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyCommand,
    ) -> Result<UpdatePartyResult, UpdatePartyError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdatePartyError::BeginTransactionFailed)?;
        let stored = self
            .parties
            .in_transaction(&mut tx)
            .find_by_id(command.party_id)
            .await?
            .ok_or(UpdatePartyError::NotFound)?;
        let mut party = stored.party;
        let outcome = apply_update(&mut party, command);

        let result = if outcome.changed() {
            self.parties
                .in_transaction(&mut tx)
                .update(&party, stored.version)
                .await?
                .into()
        } else {
            PartyDetailsView::from(crate::ports::StoredParty {
                party,
                version: stored.version,
                created: stored.created,
                updated: stored.updated,
            })
        };

        tx.commit()
            .await
            .map_err(|_| UpdatePartyError::CommitTransactionFailed)?;

        tracing::info!(
            event = "party.updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            party_id = %result.party_id,
            party_slug_id = %result.party_slug_id,
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(result)
    }
}

fn apply_update(
    party: &mut party_core::party::Party,
    command: UpdatePartyCommand,
) -> ChangeOutcome {
    let mut outcome = ChangeOutcome::Unchanged;
    if let PatchField::Set(name) = command.name {
        outcome = outcome.combine(party.rename(name));
    }

    if command.phone.is_changed() || command.email.is_changed() {
        let contact = PartyContact {
            phone: patch_option(party.contact().phone.clone(), command.phone),
            email: patch_option(party.contact().email.clone(), command.email),
        };
        outcome = outcome.combine(party.replace_contact(contact));
    }

    outcome
}

fn patch_option<T>(current: Option<T>, patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value),
        PatchField::Clear => None,
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), UpdatePartyError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map_admin_error),
        Principal::Anonymous => Err(UpdatePartyError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> UpdatePartyError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            UpdatePartyError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => UpdatePartyError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            UpdatePartyError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => UpdatePartyError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            UpdatePartyError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<PartyRepositoryError> for UpdatePartyError {
    fn from(error: PartyRepositoryError) -> Self {
        match error {
            PartyRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartyRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartyRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
