use crate::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};
use crate::use_cases::queries::get_user::GetUserRequest;
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use user_core::role::UserRole;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserAdminError {
    #[error("user not found")]
    UserNotFound,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user admin read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user admin read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user admin read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin check user admin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit check user admin transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CheckUserAdminUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError>;
}

pub struct CheckUserAdminHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> CheckUserAdminHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CheckUserAdminUseCase for CheckUserAdminHandler<U, R>
where
    U: UnitOfWork,
    R: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "check_user_admin",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CheckUserAdminError::BeginTransactionFailed)?;
        let user = self
            .reader
            .in_transaction(&mut tx)
            .find_admin_view(&GetUserRequest::ById(request.user_id))
            .await?
            .ok_or(CheckUserAdminError::UserNotFound)?;
        tx.commit()
            .await
            .map_err(|_| CheckUserAdminError::CommitTransactionFailed)?;

        if user.role != UserRole::Admin {
            return Err(CheckUserAdminError::Forbidden);
        }

        Ok(CheckUserAdminResult {
            user_id: request.user_id,
        })
    }
}

impl From<UserAdminReadError> for CheckUserAdminError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        CheckUserAdminError, CheckUserAdminHandler, CheckUserAdminRequest, CheckUserAdminUseCase,
    };
    use crate::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};
    use crate::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};
    use common::user_id::UserId;
    use serde_email::Email;
    use user_core::role::UserRole;
    use user_core::tier::UserTier;

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
        user: Option<UserDetailsView>,
        error: Option<ReadErrorKind>,
        calls: usize,
    }
    struct FakeReader {
        state: Arc<Mutex<ReadState>>,
    }

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }
    fn user_view(user_id: UserId, role: UserRole) -> UserDetailsView {
        UserDetailsView {
            user_id,
            email: email("ada@example.com"),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            prohibited_content_consent: false,
            tier: UserTier::Free,
            role,
            stripe_customer_id: None,
            structured_address: None,
            geo_address: None,
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeReader {
        async fn find_admin_view(
            &mut self,
            _request: &GetUserRequest,
        ) -> Result<Option<UserDetailsView>, UserAdminReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(ReadErrorKind::TemporarilyUnavailable) => {
                    Err(UserAdminReadError::TemporarilyUnavailable { source: boxed() })
                }
                Some(ReadErrorKind::InvalidReadModel) => {
                    Err(UserAdminReadError::InvalidReadModel { source: boxed() })
                }
                Some(ReadErrorKind::Internal) => {
                    Err(UserAdminReadError::Internal { source: boxed() })
                }
                None => Ok(state.user.clone()),
            }
        }
    }
    impl UserAdminReaderFactory<FakeTx> for FakeReadFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_check_user_admin_when_admin() {
        let user_id = UserId::new();
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(user_view(user_id, UserRole::Admin));
        assert_eq!(
            user_id,
            assert_ok(
                CheckUserAdminHandler::new(FakeUnitOfWork::default(), reads)
                    .execute(
                        &ctx(Principal::Anonymous),
                        CheckUserAdminRequest { user_id }
                    )
                    .await,
            )
            .user_id,
        );
    }

    #[tokio::test]
    async fn should_reject_missing_or_non_admin_user() {
        let user_id = UserId::new();
        let reads = FakeReadFactory::default();
        assert_error(
            CheckUserAdminHandler::new(FakeUnitOfWork::default(), reads.clone())
                .execute(&ctx(Principal::System), CheckUserAdminRequest { user_id })
                .await,
            |error| matches!(error, CheckUserAdminError::UserNotFound),
        );
        lock(&reads.state).user = Some(user_view(user_id, UserRole::User));
        assert_error(
            CheckUserAdminHandler::new(FakeUnitOfWork::default(), reads)
                .execute(&ctx(Principal::System), CheckUserAdminRequest { user_id })
                .await,
            |error| matches!(error, CheckUserAdminError::Forbidden),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_check_user_admin() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            CheckUserAdminHandler::new(begin_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), CheckUserAdminRequest { user_id })
                .await,
            |error| matches!(error, CheckUserAdminError::BeginTransactionFailed),
        );
        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(user_view(user_id, UserRole::Admin));
        assert_error(
            CheckUserAdminHandler::new(commit_uow, reads)
                .execute(&ctx(Principal::System), CheckUserAdminRequest { user_id })
                .await,
            |error| matches!(error, CheckUserAdminError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_map_read_errors_and_not_commit_for_check_user_admin() {
        for kind in [
            ReadErrorKind::TemporarilyUnavailable,
            ReadErrorKind::InvalidReadModel,
            ReadErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let reads = FakeReadFactory::default();
            lock(&reads.state).error = Some(kind);
            assert_error(
                CheckUserAdminHandler::new(uow.clone(), reads)
                    .execute(
                        &ctx(Principal::System),
                        CheckUserAdminRequest {
                            user_id: UserId::new(),
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        CheckUserAdminError::TemporarilyUnavailable { .. }
                            | CheckUserAdminError::InvalidReadModel { .. }
                            | CheckUserAdminError::Internal { .. }
                    )
                },
            );
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
