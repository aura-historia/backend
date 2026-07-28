use crate::ports::{PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory};
use common::operation_context::{OperationContext, Principal};
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, user_id::UserId};

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
    TemporarilyUnavailable,
    #[error("invalid partner shop read model")]
    InvalidReadModel,
    #[error("internal partner shop read failure")]
    Internal,
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

impl From<PartnerShopReadError> for CheckUserPartnerShopError {
    fn from(error: PartnerShopReadError) -> Self {
        match error {
            PartnerShopReadError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            PartnerShopReadError::InvalidReadModel => Self::InvalidReadModel,
            PartnerShopReadError::Internal => Self::Internal,
        }
    }
}

fn authorize_check(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), CheckUserPartnerShopError> {
    match &context.principal {
        Principal::User(user_id) if *user_id == requested_user_id => Ok(()),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous | Principal::User(_) => Err(CheckUserPartnerShopError::Forbidden),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, RequestId};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct TestPartnerShopReaderFactory {
        called: Arc<Mutex<bool>>,
        is_partner: bool,
    }

    struct TestPartnerShopReader {
        called: Arc<Mutex<bool>>,
        is_partner: bool,
    }

    struct TestUnitOfWork {
        committed: Arc<Mutex<bool>>,
    }

    struct TestTransaction {
        committed: Arc<Mutex<bool>>,
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, common::transaction::TransactionError> {
            Ok(TestTransaction {
                committed: Arc::clone(&self.committed),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), common::transaction::TransactionError> {
            with_mutex(&self.committed, |committed| *committed = true);
            Ok(())
        }
    }

    impl PartnerShopReaderFactory<TestTransaction> for TestPartnerShopReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl PartnerShopReader + 'tx {
            TestPartnerShopReader {
                called: Arc::clone(&self.called),
                is_partner: self.is_partner,
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopReader for TestPartnerShopReader {
        async fn is_user_partner_of_shop(
            &mut self,
            _request: &CheckUserPartnerShopRequest,
        ) -> Result<bool, PartnerShopReadError> {
            with_mutex(&self.called, |called| *called = true);
            Ok(self.is_partner)
        }
    }

    #[tokio::test]
    async fn should_check_partner_shop_in_owned_transaction() {
        let user_id = UserId::new();
        let shop_id = ShopId::new();
        let committed = Arc::new(Mutex::new(false));
        let called = Arc::new(Mutex::new(false));
        let handler = CheckUserPartnerShopHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestPartnerShopReaderFactory {
                called: Arc::clone(&called),
                is_partner: true,
            },
        );

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
        assert!(with_mutex(&called, |called| *called));
        assert!(with_mutex(&committed, |committed| *committed));
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
    fn should_reject_user_checking_other_user() {
        let context = OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        };

        let result = authorize_check(&context, UserId::new());

        assert!(matches!(result, Err(CheckUserPartnerShopError::Forbidden)));
    }

    fn user_context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn with_mutex<T, R>(mutex: &Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
        match mutex.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
