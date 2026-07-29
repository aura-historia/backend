use crate::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::transaction::{Transaction, UnitOfWork};
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use serde_json::Value;
use user_core::{role::UserRole, tier::UserTier, user_search::UserSearch};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersRequest {
    pub search: UserSearch,
    pub sort: Option<Sort<user_core::sort_user_field::SortUserField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<user_core::first_name::FirstName>,
    pub last_name: Option<user_core::last_name::LastName>,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersResult {
    pub items: Vec<UserSummary>,
    pub cursor: Cursor<Value>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchUsersError {
    #[error("temporary user search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search users transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search users transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchUsersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError>;
}

pub struct SearchUsersHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchUsersHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchUsersUseCase for SearchUsersHandler<U, R>
where
    U: UnitOfWork,
    R: UserSearchReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_users",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchUsersError::BeginTransactionFailed)?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchUsersError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserSearchReadError> for SearchUsersError {
    fn from(error: UserSearchReadError) -> Self {
        match error {
            UserSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserSearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        SearchUsersError, SearchUsersHandler, SearchUsersRequest, SearchUsersResult,
        SearchUsersUseCase,
    };
    use crate::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
    use common::pagination::cursor::Cursor;
    use common::user_id::UserId;
    use serde_json::Value;
    use user_core::user_search::UserSearch;

    use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use common::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct TxState {
        begin_error: bool,
        commit_error: bool,
        begins: usize,
        commits: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<TxState>>,
    }

    struct FakeTx {
        state: Arc<Mutex<TxState>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn ctx(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn assert_error<T, E, F>(result: Result<T, E>, predicate: F)
    where
        E: Debug,
        F: FnOnce(&E) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    fn assert_ok<T, E: Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected ok, got {error:?}"),
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                state.commits += 1;
                Ok(())
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.state);
            state.begins += 1;
            if state.begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    use common::error::boxed::{BoxError, box_error};

    #[derive(Debug, Clone, Copy)]
    enum ReadErrorKind {
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    #[derive(Clone, Default)]
    struct FakeReadFactory {
        state: Arc<Mutex<ReadState>>,
    }
    #[derive(Default)]
    struct ReadState {
        result: Option<SearchUsersResult>,
        error: Option<ReadErrorKind>,
        calls: usize,
    }
    struct FakeReader {
        state: Arc<Mutex<ReadState>>,
    }

    #[async_trait::async_trait]
    impl UserSearchReader for FakeReader {
        async fn search(
            &mut self,
            _request: &SearchUsersRequest,
        ) -> Result<SearchUsersResult, UserSearchReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(ReadErrorKind::TemporarilyUnavailable) => {
                    Err(UserSearchReadError::TemporarilyUnavailable { source: boxed() })
                }
                Some(ReadErrorKind::InvalidReadModel) => {
                    Err(UserSearchReadError::InvalidReadModel { source: boxed() })
                }
                Some(ReadErrorKind::Internal) => {
                    Err(UserSearchReadError::Internal { source: boxed() })
                }
                None => Ok(match state.result.clone() {
                    Some(result) => result,
                    None => SearchUsersResult {
                        items: Vec::new(),
                        cursor: Cursor::<Value>::default(),
                        total: Some(0),
                    },
                }),
            }
        }
    }
    impl UserSearchReaderFactory<FakeTx> for FakeReadFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserSearchReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    fn request() -> SearchUsersRequest {
        SearchUsersRequest {
            search: UserSearch::default(),
            sort: None,
            cursor: None,
        }
    }

    #[tokio::test]
    async fn should_search_users_when_read_succeeds() {
        let reads = FakeReadFactory::default();
        lock(&reads.state).result = Some(SearchUsersResult {
            items: Vec::new(),
            cursor: Cursor::default(),
            total: Some(0),
        });
        assert_eq!(
            0,
            assert_ok(
                SearchUsersHandler::new(FakeUnitOfWork::default(), reads)
                    .execute(&ctx(Principal::System), request())
                    .await,
            )
            .items
            .len(),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_search_users() {
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            SearchUsersHandler::new(begin_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), request())
                .await,
            |error| matches!(error, SearchUsersError::BeginTransactionFailed),
        );
        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        assert_error(
            SearchUsersHandler::new(commit_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), request())
                .await,
            |error| matches!(error, SearchUsersError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_map_read_errors_and_not_commit_for_search_users() {
        for kind in [
            ReadErrorKind::TemporarilyUnavailable,
            ReadErrorKind::InvalidReadModel,
            ReadErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let reads = FakeReadFactory::default();
            lock(&reads.state).error = Some(kind);
            assert_error(
                SearchUsersHandler::new(uow.clone(), reads)
                    .execute(&ctx(Principal::System), request())
                    .await,
                |error| {
                    matches!(
                        error,
                        SearchUsersError::TemporarilyUnavailable { .. }
                            | SearchUsersError::InvalidReadModel { .. }
                            | SearchUsersError::Internal { .. }
                    )
                },
            );
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
