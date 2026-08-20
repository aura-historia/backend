use crate::ports::{
    UserStripeCustomerReadError, UserStripeCustomerReader, UserStripeCustomerReaderFactory,
};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use user_core::{role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq)]
pub struct FindUserByStripeCustomerIdRequest {
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserStripeLookupView {
    pub user_id: UserId,
    pub email: Email,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, thiserror::Error)]
pub enum FindUserByStripeCustomerIdError {
    #[error("user not found")]
    NotFound,
    #[error("temporary user stripe customer lookup failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user stripe customer read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user stripe customer lookup failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin find user by stripe customer id transaction")]
    BeginTransactionFailed,
    #[error("failed to commit find user by stripe customer id transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait FindUserByStripeCustomerIdUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: FindUserByStripeCustomerIdRequest,
    ) -> Result<UserStripeLookupView, FindUserByStripeCustomerIdError>;
}

pub struct FindUserByStripeCustomerIdHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> FindUserByStripeCustomerIdHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> FindUserByStripeCustomerIdUseCase for FindUserByStripeCustomerIdHandler<U, R>
where
    U: UnitOfWork,
    R: UserStripeCustomerReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "find_user_by_stripe_customer_id",
        skip_all,
        fields(
            stripe_customer_id = %request.stripe_customer_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: FindUserByStripeCustomerIdRequest,
    ) -> Result<UserStripeLookupView, FindUserByStripeCustomerIdError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| FindUserByStripeCustomerIdError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_by_stripe_customer_id(&request)
            .await?
            .ok_or(FindUserByStripeCustomerIdError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| FindUserByStripeCustomerIdError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserStripeCustomerReadError> for FindUserByStripeCustomerIdError {
    fn from(error: UserStripeCustomerReadError) -> Self {
        match error {
            UserStripeCustomerReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserStripeCustomerReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            UserStripeCustomerReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        FindUserByStripeCustomerIdError, FindUserByStripeCustomerIdHandler,
        FindUserByStripeCustomerIdRequest, FindUserByStripeCustomerIdUseCase, UserStripeLookupView,
    };
    use crate::ports::{
        UserStripeCustomerReadError, UserStripeCustomerReader, UserStripeCustomerReaderFactory,
    };
    use common::stripe_customer_id::StripeCustomerId;
    use common::user_id::UserId;
    use serde_email::Email;
    use user_core::role::UserRole;
    use user_core::tier::UserTier;

    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
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
        user: Option<UserStripeLookupView>,
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

    fn stripe_view(user_id: UserId) -> UserStripeLookupView {
        UserStripeLookupView {
            user_id,
            email: email("ada@example.com"),
            tier: UserTier::Free,
            role: UserRole::User,
            stripe_customer_id: StripeCustomerId::from("cus_123"),
        }
    }

    #[async_trait::async_trait]
    impl UserStripeCustomerReader for FakeReader {
        async fn find_by_stripe_customer_id(
            &mut self,
            _request: &FindUserByStripeCustomerIdRequest,
        ) -> Result<Option<UserStripeLookupView>, UserStripeCustomerReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(ReadErrorKind::TemporarilyUnavailable) => {
                    Err(UserStripeCustomerReadError::TemporarilyUnavailable { source: boxed() })
                }
                Some(ReadErrorKind::InvalidReadModel) => {
                    Err(UserStripeCustomerReadError::InvalidReadModel { source: boxed() })
                }
                Some(ReadErrorKind::Internal) => {
                    Err(UserStripeCustomerReadError::Internal { source: boxed() })
                }
                None => Ok(state.user.clone()),
            }
        }
    }
    impl UserStripeCustomerReaderFactory<FakeTx> for FakeReadFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl UserStripeCustomerReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    fn request() -> FindUserByStripeCustomerIdRequest {
        FindUserByStripeCustomerIdRequest {
            stripe_customer_id: StripeCustomerId::from("cus_123"),
        }
    }

    #[tokio::test]
    async fn should_find_user_by_stripe_customer_id_when_found() {
        let user_id = UserId::new();
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(stripe_view(user_id));
        assert_eq!(
            user_id,
            assert_ok(
                FindUserByStripeCustomerIdHandler::new(FakeUnitOfWork::default(), reads)
                    .execute(&ctx(Principal::System), request())
                    .await,
            )
            .user_id,
        );
    }

    #[tokio::test]
    async fn should_return_not_found_when_stripe_customer_missing() {
        assert_error(
            FindUserByStripeCustomerIdHandler::new(
                FakeUnitOfWork::default(),
                FakeReadFactory::default(),
            )
            .execute(&ctx(Principal::System), request())
            .await,
            |error| matches!(error, FindUserByStripeCustomerIdError::NotFound),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_find_user_by_stripe_customer_id() {
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            FindUserByStripeCustomerIdHandler::new(begin_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), request())
                .await,
            |error| {
                matches!(
                    error,
                    FindUserByStripeCustomerIdError::BeginTransactionFailed
                )
            },
        );
        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let reads = FakeReadFactory::default();
        lock(&reads.state).user = Some(stripe_view(UserId::new()));
        assert_error(
            FindUserByStripeCustomerIdHandler::new(commit_uow, reads)
                .execute(&ctx(Principal::System), request())
                .await,
            |error| {
                matches!(
                    error,
                    FindUserByStripeCustomerIdError::CommitTransactionFailed
                )
            },
        );
    }

    #[tokio::test]
    async fn should_map_read_errors_and_not_commit_for_find_user_by_stripe_customer_id() {
        for kind in [
            ReadErrorKind::TemporarilyUnavailable,
            ReadErrorKind::InvalidReadModel,
            ReadErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let reads = FakeReadFactory::default();
            lock(&reads.state).error = Some(kind);
            assert_error(
                FindUserByStripeCustomerIdHandler::new(uow.clone(), reads)
                    .execute(&ctx(Principal::System), request())
                    .await,
                |error| {
                    matches!(
                        error,
                        FindUserByStripeCustomerIdError::TemporarilyUnavailable { .. }
                            | FindUserByStripeCustomerIdError::InvalidReadModel { .. }
                            | FindUserByStripeCustomerIdError::Internal { .. }
                    )
                },
            );
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
