use crate::ports::{PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use shop_core::shop_id::ShopId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopRequest {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
    pub is_partner: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserPartnerShopError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partner shop read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin check user partner shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit check user partner shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CheckUserPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError>;
}

pub struct CheckUserPartnerShopHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> CheckUserPartnerShopHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CheckUserPartnerShopUseCase for CheckUserPartnerShopHandler<U, R>
where
    U: UnitOfWork,
    R: PartnerShopReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "check_user_partner_shop",
        skip_all,
        fields(
            user_id = %request.user_id,
            shop_id = %request.shop_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError> {
        authorize_check(context, request.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CheckUserPartnerShopError::BeginTransactionFailed)?;

        let is_partner = self
            .reader
            .in_transaction(&mut tx)
            .is_user_partner_of_shop(&request)
            .await?;

        tx.commit()
            .await
            .map_err(|_| CheckUserPartnerShopError::CommitTransactionFailed)?;

        Ok(CheckUserPartnerShopResult {
            user_id: request.user_id,
            shop_id: request.shop_id,
            is_partner,
        })
    }
}

impl From<OperationAuthorizationError> for CheckUserPartnerShopError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => Self::Forbidden,
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopReadError> for CheckUserPartnerShopError {
    fn from(error: PartnerShopReadError) -> Self {
        match error {
            PartnerShopReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            PartnerShopReadError::Internal { source } => Self::Internal { source },
        }
    }
}

fn authorize_check(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), CheckUserPartnerShopError> {
    context
        .require()
        .credential_capability(CredentialCapability::PartnerShopsRead)
        .user(&requested_user_id)
        .service_or_system()
        .authorize::<CheckUserPartnerShopError>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use application::operation_context::{
        CorrelationId, CredentialCapability, Principal, RequestId,
    };
    use application::transaction::{TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum ReadErrorKind {
        Internal,
    }

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        partner_read: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        partner_read: bool,
        partner_read_error: Option<ReadErrorKind>,
        last_partner_request: Option<CheckUserPartnerShopRequest>,
        counts: Counts,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakePartnerReaderFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakePartnerReader {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.begin += 1;
                state.begin_error
            });
            if fail {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.commit += 1;
                state.commit_error
            });
            if fail {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl PartnerShopReaderFactory<FakeTx> for FakePartnerReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl PartnerShopReader + 'tx {
            FakePartnerReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopReader for FakePartnerReader {
        async fn is_user_partner_of_shop(
            &mut self,
            request: &CheckUserPartnerShopRequest,
        ) -> Result<bool, PartnerShopReadError> {
            with_state(&self.state, |state| {
                state.counts.partner_read += 1;
                state.last_partner_request = Some(request.clone());
                match state.partner_read_error {
                    Some(kind) => Err(partner_read_error(kind)),
                    None => Ok(state.partner_read),
                }
            })
        }

        async fn list_summaries_for_user(
            &mut self,
            _user_id: UserId,
        ) -> Result<Vec<crate::use_cases::queries::search_shops::ShopSummary>, PartnerShopReadError>
        {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn should_check_user_partner_shop_when_authorized() {
        let state = shared_state();
        let user_id = UserId::new();
        let shop_id = ShopId::new();
        with_state(&state, |state| state.partner_read = true);
        let handler = CheckUserPartnerShopHandler::new(uow(&state), partner_reader(&state));

        let result = handler
            .execute(
                &user_context(user_id),
                CheckUserPartnerShopRequest { user_id, shop_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(CheckUserPartnerShopResult {
                is_partner: true,
                ..
            })
        ));
        assert_eq!(
            Some(CheckUserPartnerShopRequest { user_id, shop_id }),
            with_state(&state, |state| state.last_partner_request.clone())
        );
        assert_counts(&state, |counts| assert_eq!(1, counts.commit));
    }

    #[tokio::test]
    async fn should_forbid_partner_check_before_begin_when_wrong_user_or_anonymous() {
        let state = shared_state();
        let handler = CheckUserPartnerShopHandler::new(uow(&state), partner_reader(&state));

        let wrong_user = handler
            .execute(
                &user_context(UserId::new()),
                CheckUserPartnerShopRequest {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;

        assert!(matches!(
            wrong_user,
            Err(CheckUserPartnerShopError::Forbidden)
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.begin));

        let anonymous = handler
            .execute(
                &anonymous_context(),
                CheckUserPartnerShopRequest {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            anonymous,
            Err(CheckUserPartnerShopError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn should_cover_partner_check_errors_and_service_auth() {
        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler = CheckUserPartnerShopHandler::new(uow(&state), partner_reader(&state));
        let begin = handler
            .execute(
                &service_context(),
                CheckUserPartnerShopRequest {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            begin,
            Err(CheckUserPartnerShopError::BeginTransactionFailed)
        ));

        let state = shared_state();
        with_state(&state, |state| {
            state.partner_read_error = Some(ReadErrorKind::Internal)
        });
        let handler = CheckUserPartnerShopHandler::new(uow(&state), partner_reader(&state));
        let read = handler
            .execute(
                &service_context(),
                CheckUserPartnerShopRequest {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            read,
            Err(CheckUserPartnerShopError::Internal { .. })
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.commit_error = true);
        let handler = CheckUserPartnerShopHandler::new(uow(&state), partner_reader(&state));
        let commit = handler
            .execute(
                &service_context(),
                CheckUserPartnerShopRequest {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            commit,
            Err(CheckUserPartnerShopError::CommitTransactionFailed)
        ));
    }

    #[test]
    fn should_allow_user_to_check_own_partner_shop() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, user_id);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn should_allow_delegated_user_to_check_own_partner_shop() {
        let user_id = UserId::new();
        let context = OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::PartnerShopsRead]),
            },
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, user_id);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn should_reject_user_checking_other_user() {
        let context = OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, UserId::new());

        assert!(matches!(result, Err(CheckUserPartnerShopError::Forbidden)));
    }

    #[test]
    fn should_reject_delegated_user_checking_other_user() {
        let context = OperationContext {
            principal: Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::new(),
            },
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, UserId::new());

        assert!(matches!(result, Err(CheckUserPartnerShopError::Forbidden)));
    }

    fn partner_reader(state: &Arc<Mutex<State>>) -> FakePartnerReaderFactory {
        FakePartnerReaderFactory {
            state: Arc::clone(state),
        }
    }

    fn uow(state: &Arc<Mutex<State>>) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn shared_state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State::default()))
    }

    fn partner_read_error(kind: ReadErrorKind) -> PartnerShopReadError {
        match kind {
            ReadErrorKind::Internal => PartnerShopReadError::Internal {
                source: static_error("internal"),
            },
        }
    }

    fn service_context() -> OperationContext {
        OperationContext {
            principal: Principal::Service("shop-service-test".to_string()),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn user_context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn anonymous_context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn assert_counts(state: &Arc<Mutex<State>>, assert: impl FnOnce(&Counts)) {
        with_state(state, |state| assert(&state.counts));
    }

    fn with_state<R>(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State) -> R) -> R {
        match state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
