use crate::ports::{UserAccountReadError, UserAccountReader, UserAccountReaderFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
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
pub enum GetUserError {
    #[error("user not found")]
    NotFound,
    #[error("temporary user account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user account read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user account read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetUserRequest,
    ) -> Result<UserDetailsView, GetUserError>;
}

pub struct GetUserHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetUserHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetUserUseCase for GetUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_user",
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
        request: GetUserRequest,
    ) -> Result<UserDetailsView, GetUserError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetUserError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_account(&request)
            .await?
            .ok_or(GetUserError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetUserError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserAccountReadError> for GetUserError {
    fn from(error: UserAccountReadError) -> Self {
        match error {
            UserAccountReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAccountReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAccountReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{GetUserError, GetUserHandler, GetUserRequest, GetUserUseCase, UserDetailsView};
    use crate::ports::{UserAccountReadError, UserAccountReader, UserAccountReaderFactory};
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
    impl UserAccountReader for FakeReader {
        async fn find_account(
            &mut self,
            _request: &GetUserRequest,
        ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(ReadErrorKind::TemporarilyUnavailable) => {
                    Err(UserAccountReadError::TemporarilyUnavailable { source: boxed() })
                }
                Some(ReadErrorKind::InvalidReadModel) => {
                    Err(UserAccountReadError::InvalidReadModel { source: boxed() })
                }
                Some(ReadErrorKind::Internal) => {
                    Err(UserAccountReadError::Internal { source: boxed() })
                }
                None => Ok(state.user.clone()),
            }
        }
    }

    impl UserAccountReaderFactory<FakeTx> for FakeReadFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAccountReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_get_user_when_found() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(user_view(user_id, UserRole::Admin));

        assert_eq!(
            user_id,
            assert_ok(
                GetUserHandler::new(uow.clone(), reads)
                    .execute(
                        &ctx(Principal::Anonymous),
                        GetUserRequest::ByEmail(email("ada@example.com"))
                    )
                    .await,
            )
            .user_id,
        );
        assert_eq!(1, lock(&uow.state).commits);
    }

    #[tokio::test]
    async fn should_return_not_found_when_user_missing() {
        assert_error(
            GetUserHandler::new(FakeUnitOfWork::default(), FakeReadFactory::default())
                .execute(&ctx(Principal::System), GetUserRequest::ById(UserId::new()))
                .await,
            |error| matches!(error, GetUserError::NotFound),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_get_user() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            GetUserHandler::new(begin_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), GetUserRequest::ById(user_id))
                .await,
            |error| matches!(error, GetUserError::BeginTransactionFailed),
        );

        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(user_view(user_id, UserRole::Admin));
        assert_error(
            GetUserHandler::new(commit_uow, reads)
                .execute(&ctx(Principal::System), GetUserRequest::ById(user_id))
                .await,
            |error| matches!(error, GetUserError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_map_read_errors_and_not_commit_for_get_user() {
        for kind in [
            ReadErrorKind::TemporarilyUnavailable,
            ReadErrorKind::InvalidReadModel,
            ReadErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let reads = FakeReadFactory::default();
            lock(&reads.state).error = Some(kind);
            assert_error(
                GetUserHandler::new(uow.clone(), reads)
                    .execute(&ctx(Principal::System), GetUserRequest::ById(UserId::new()))
                    .await,
                |error| {
                    matches!(
                        error,
                        GetUserError::TemporarilyUnavailable { .. }
                            | GetUserError::InvalidReadModel { .. }
                            | GetUserError::Internal { .. }
                    )
                },
            );
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
