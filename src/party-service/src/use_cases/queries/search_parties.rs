use crate::ports::{PartySearchReadError, PartySearchReader, PartySearchReaderFactory};
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::sort::Sort;
use party_core::party::PartyContact;
use party_core::party_id::PartyId;
use party_core::party_name::PartyName;
use party_core::party_search::PartySearch;
use party_core::party_slug_id::PartySlugId;
use party_core::sort_party_field::SortPartyField;

use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchPartiesRequest {
    pub search: PartySearch,
    pub sort: Option<Sort<SortPartyField>>,
    pub cursor: Option<Cursor<PartyId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartySummary {
    pub party_id: PartyId,
    pub party_slug_id: PartySlugId,
    pub name: PartyName,
    pub contact: PartyContact,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchPartiesResult {
    pub items: Vec<PartySummary>,
    pub cursor: Cursor<PartyId>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchPartiesError {
    #[error("authenticated actor required to search parties")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary party search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid party search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal party search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search parties transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search parties transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchPartiesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchPartiesRequest,
    ) -> Result<SearchPartiesResult, SearchPartiesError>;
}

pub struct SearchPartiesHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    check_user_admin: A,
}

impl<U, R, A> SearchPartiesHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            reader,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> SearchPartiesUseCase for SearchPartiesHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartySearchReaderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "search_parties",
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
        request: SearchPartiesRequest,
    ) -> Result<SearchPartiesResult, SearchPartiesError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchPartiesError::BeginTransactionFailed)?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchPartiesError::CommitTransactionFailed)?;

        Ok(result)
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), SearchPartiesError>
where
    A: CheckUserAdminUseCase,
{
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map_admin_error),
        Principal::Anonymous => Err(SearchPartiesError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> SearchPartiesError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            SearchPartiesError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => SearchPartiesError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            SearchPartiesError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => SearchPartiesError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            SearchPartiesError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<PartySearchReadError> for SearchPartiesError {
    fn from(error: PartySearchReadError) -> Self {
        match error {
            PartySearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartySearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            PartySearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{PartySearchReader, PartySearchReaderFactory};
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex};
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        begins: usize,
        searches: usize,
        commits: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.begins += 1;
            drop(state);
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    #[derive(Clone)]
    struct FakeReaderFactory(Arc<Mutex<State>>);

    struct FakeReader(Arc<Mutex<State>>);

    impl PartySearchReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartySearchReader + 'tx {
            FakeReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl PartySearchReader for FakeReader {
        async fn search(
            &mut self,
            _request: &SearchPartiesRequest,
        ) -> Result<SearchPartiesResult, PartySearchReadError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.searches += 1;
            drop(state);
            Ok(SearchPartiesResult {
                items: vec![],
                cursor: Cursor::default(),
                total: None,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAdmin {
        allowed: bool,
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for FakeAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.allowed {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request() -> SearchPartiesRequest {
        SearchPartiesRequest {
            search: PartySearch::default(),
            sort: None,
            cursor: None,
        }
    }

    #[tokio::test]
    async fn should_authorize_admin_before_searching_and_commit_result() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchPartiesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: true },
        );

        let result = handler
            .execute(&context(Principal::User(UserId::new())), request())
            .await;

        assert!(result.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(1, state.searches);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_beginning_party_search_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchPartiesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: false },
        );

        let result = handler
            .execute(&context(Principal::User(UserId::new())), request())
            .await;

        assert!(matches!(result, Err(SearchPartiesError::Forbidden)));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(0, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_checking_admin_or_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchPartiesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: true },
        );

        let result = handler
            .execute(&context(Principal::Anonymous), request())
            .await;

        assert!(matches!(
            result,
            Err(SearchPartiesError::AuthenticatedActorRequired)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(0, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }
}
