use crate::ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory, StoredParty};
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use party_core::{
    party::PartyContact, party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId,
};
use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub enum GetPartyRequest {
    ById(PartyId),
    BySlug(PartySlugId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartyDetailsView {
    pub party_id: PartyId,
    pub party_slug_id: PartySlugId,
    pub name: PartyName,
    pub contact: PartyContact,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl From<StoredParty> for PartyDetailsView {
    fn from(stored: StoredParty) -> Self {
        Self {
            party_id: stored.party.id(),
            party_slug_id: stored.party.slug_id().clone(),
            name: stored.party.name().clone(),
            contact: stored.party.contact().clone(),
            created: stored.created,
            updated: stored.updated,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GetPartyError {
    #[error("authenticated actor required to get party")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party not found")]
    NotFound,
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
    #[error("failed to begin get party transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get party transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetPartyUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartyRequest,
    ) -> Result<PartyDetailsView, GetPartyError>;
}

pub struct GetPartyHandler<U, R, A> {
    unit_of_work: U,
    parties: R,
    check_user_admin: A,
}

impl<U, R, A> GetPartyHandler<U, R, A> {
    pub fn new(unit_of_work: U, parties: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> GetPartyUseCase for GetPartyHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "get_party",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartyRequest,
    ) -> Result<PartyDetailsView, GetPartyError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetPartyError::BeginTransactionFailed)?;
        let result = match request {
            GetPartyRequest::ById(id) => {
                self.parties.in_transaction(&mut tx).find_by_id(id).await?
            }
            GetPartyRequest::BySlug(slug_id) => {
                self.parties
                    .in_transaction(&mut tx)
                    .find_by_slug(&slug_id)
                    .await?
            }
        }
        .ok_or(GetPartyError::NotFound)?
        .into();

        tx.commit()
            .await
            .map_err(|_| GetPartyError::CommitTransactionFailed)?;

        Ok(result)
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), GetPartyError>
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
        Principal::Anonymous => Err(GetPartyError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> GetPartyError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            GetPartyError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => GetPartyError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            GetPartyError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => GetPartyError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => GetPartyError::TemporarilyUnavailable {
            source: static_error("check user admin transaction failed"),
        },
    }
}

impl From<PartyRepositoryError> for GetPartyError {
    fn from(error: PartyRepositoryError) -> Self {
        match error {
            PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartyRepositoryError::Internal { source }
            | PartyRepositoryError::SlugConflict { source } => Self::Internal { source },
            PartyRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected party read concurrency conflict"),
            },
        }
    }
}
