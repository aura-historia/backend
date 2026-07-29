use crate::ports::{
    UserPartnerShopsReadError, UserPartnerShopsReader, UserPartnerShopsReaderFactory,
};
use common::error::boxed::BoxError;
use common::operation_context::{OperationContext, Principal};
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsResult {
    pub user_id: UserId,
    pub items: Vec<PartnerShopSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListPartnerShopsError {
    #[error("user not found")]
    UserNotFound,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user partner shops read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user partner shops read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user partner shops read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list partner shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit list partner shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListPartnerShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError>;
}

pub struct ListPartnerShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> ListPartnerShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListPartnerShopsUseCase for ListPartnerShopsHandler<U, R>
where
    U: UnitOfWork,
    R: UserPartnerShopsReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_partner_shops",
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
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError> {
        authorize_list(context, request.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListPartnerShopsError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .list_partner_shops(&request)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListPartnerShopsError::CommitTransactionFailed)?;

        Ok(result)
    }
}

fn authorize_list(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), ListPartnerShopsError> {
    match &context.principal {
        Principal::User(user_id) if *user_id == requested_user_id => Ok(()),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous | Principal::User(_) => Err(ListPartnerShopsError::Forbidden),
    }
}

impl From<UserPartnerShopsReadError> for ListPartnerShopsError {
    fn from(error: UserPartnerShopsReadError) -> Self {
        match error {
            UserPartnerShopsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserPartnerShopsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            UserPartnerShopsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        ListPartnerShopsError, ListPartnerShopsHandler, ListPartnerShopsRequest,
        ListPartnerShopsResult, ListPartnerShopsUseCase,
    };
    use crate::ports::{
        UserPartnerShopsReadError, UserPartnerShopsReader, UserPartnerShopsReaderFactory,
    };
    use common::user_id::UserId;

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
        result: Option<ListPartnerShopsResult>,
        error: Option<ReadErrorKind>,
        calls: usize,
    }
    struct FakeReader {
        state: Arc<Mutex<ReadState>>,
    }

    #[async_trait::async_trait]
    impl UserPartnerShopsReader for FakeReader {
        async fn list_partner_shops(
            &mut self,
            _request: &ListPartnerShopsRequest,
        ) -> Result<ListPartnerShopsResult, UserPartnerShopsReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            match state.error {
                Some(ReadErrorKind::TemporarilyUnavailable) => {
                    Err(UserPartnerShopsReadError::TemporarilyUnavailable { source: boxed() })
                }
                Some(ReadErrorKind::InvalidReadModel) => {
                    Err(UserPartnerShopsReadError::InvalidReadModel { source: boxed() })
                }
                Some(ReadErrorKind::Internal) => {
                    Err(UserPartnerShopsReadError::Internal { source: boxed() })
                }
                None => Ok(match state.result.clone() {
                    Some(result) => result,
                    None => ListPartnerShopsResult {
                        user_id: UserId::new(),
                        items: Vec::new(),
                    },
                }),
            }
        }
    }
    impl UserPartnerShopsReaderFactory<FakeTx> for FakeReadFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl UserPartnerShopsReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_list_partner_shops_when_authorized() {
        let user_id = UserId::new();
        let reads = FakeReadFactory::default();
        lock(&reads.state).result = Some(ListPartnerShopsResult {
            user_id,
            items: Vec::new(),
        });
        assert_eq!(
            user_id,
            assert_ok(
                ListPartnerShopsHandler::new(FakeUnitOfWork::default(), reads)
                    .execute(
                        &ctx(Principal::User(user_id)),
                        ListPartnerShopsRequest { user_id }
                    )
                    .await,
            )
            .user_id,
        );
    }

    #[tokio::test]
    async fn should_reject_unauthorized_partner_shop_list() {
        let user_id = UserId::new();
        let reads = FakeReadFactory::default();
        assert_error(
            ListPartnerShopsHandler::new(FakeUnitOfWork::default(), reads.clone())
                .execute(
                    &ctx(Principal::User(UserId::new())),
                    ListPartnerShopsRequest { user_id },
                )
                .await,
            |error| matches!(error, ListPartnerShopsError::Forbidden),
        );
        assert_error(
            ListPartnerShopsHandler::new(FakeUnitOfWork::default(), reads)
                .execute(
                    &ctx(Principal::Anonymous),
                    ListPartnerShopsRequest { user_id },
                )
                .await,
            |error| matches!(error, ListPartnerShopsError::Forbidden),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_list_partner_shops() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            ListPartnerShopsHandler::new(begin_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), ListPartnerShopsRequest { user_id })
                .await,
            |error| matches!(error, ListPartnerShopsError::BeginTransactionFailed),
        );
        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        assert_error(
            ListPartnerShopsHandler::new(commit_uow, FakeReadFactory::default())
                .execute(&ctx(Principal::System), ListPartnerShopsRequest { user_id })
                .await,
            |error| matches!(error, ListPartnerShopsError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_map_read_errors_and_not_commit_for_list_partner_shops() {
        for kind in [
            ReadErrorKind::TemporarilyUnavailable,
            ReadErrorKind::InvalidReadModel,
            ReadErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let reads = FakeReadFactory::default();
            lock(&reads.state).error = Some(kind);
            assert_error(
                ListPartnerShopsHandler::new(uow.clone(), reads)
                    .execute(
                        &ctx(Principal::System),
                        ListPartnerShopsRequest {
                            user_id: UserId::new(),
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        ListPartnerShopsError::TemporarilyUnavailable { .. }
                            | ListPartnerShopsError::InvalidReadModel { .. }
                            | ListPartnerShopsError::Internal { .. }
                    )
                },
            );
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
