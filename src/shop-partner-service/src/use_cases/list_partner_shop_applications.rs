use crate::ports::{
    PartnerShopApplicationReader, PartnerShopApplicationReaderFactory,
    PartnerShopApplicationRepositoryError, PartnerShopApplicationView,
};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopApplicationsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopApplicationsResult {
    pub items: Vec<PartnerShopApplicationView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListPartnerShopApplicationsError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partner shop application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin partner shop application transaction")]
    BeginTransactionFailed,
    #[error("failed to commit partner shop application transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListPartnerShopApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopApplicationsRequest,
    ) -> Result<ListPartnerShopApplicationsResult, ListPartnerShopApplicationsError>;
}

pub struct ListPartnerShopApplicationsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> ListPartnerShopApplicationsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListPartnerShopApplicationsUseCase for ListPartnerShopApplicationsHandler<U, R>
where
    U: UnitOfWork,
    R: PartnerShopApplicationReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "list_partner_shop_applications", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopApplicationsRequest,
    ) -> Result<ListPartnerShopApplicationsResult, ListPartnerShopApplicationsError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
            .user(&request.user_id)
            .service_or_system()
            .authorize::<ListPartnerShopApplicationsError>()?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListPartnerShopApplicationsError::BeginTransactionFailed)?;
        let items = self
            .reader
            .in_transaction(&mut tx)
            .list_by_user(request.user_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListPartnerShopApplicationsError::CommitTransactionFailed)?;
        Ok(ListPartnerShopApplicationsResult { items })
    }
}

impl From<OperationAuthorizationError> for ListPartnerShopApplicationsError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for ListPartnerShopApplicationsError {
    fn from(error: PartnerShopApplicationRepositoryError) -> Self {
        match error {
            PartnerShopApplicationRepositoryError::ConcurrencyConflict => Self::Internal {
                source: "unexpected read concurrency conflict".into(),
            },
            PartnerShopApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnerShopApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnerShopApplicationReader, PartnerShopApplicationReaderFactory,
        PartnerShopApplicationRepositoryError, PartnerShopApplicationStorageVersion,
        VersionedPartnerShopApplication,
    };
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::transaction::{TransactionError, UnitOfWork};
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId};
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TestUnitOfWork {
        state: SharedState,
    }

    struct TestTransaction {
        state: SharedState,
    }

    #[derive(Clone, Default)]
    struct TestApplicationFactory {
        state: SharedState,
    }

    #[derive(Clone, Default)]
    struct SharedState {
        applications: Arc<Mutex<Vec<VersionedPartnerShopApplication>>>,
        committed: Arc<Mutex<bool>>,
    }

    struct TestApplicationPort {
        state: SharedState,
    }

    impl SharedState {
        fn with_application(application: PartnerShopApplication) -> Self {
            let state = Self::default();
            state.push(application);
            state
        }

        fn push(&self, application: PartnerShopApplication) {
            if let Ok(mut applications) = self.applications.lock() {
                applications.push(VersionedPartnerShopApplication::new(
                    application,
                    PartnerShopApplicationStorageVersion::INITIAL,
                ));
            }
        }

        fn committed(&self) -> bool {
            self.committed.lock().map(|value| *value).unwrap_or(false)
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            self.state
                .committed
                .lock()
                .map(|mut committed| *committed = true)
                .map_err(|_| TransactionError::CommitFailed)
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTransaction {
                state: self.state.clone(),
            })
        }
    }

    impl<Tx> PartnerShopApplicationReaderFactory<Tx> for TestApplicationFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl PartnerShopApplicationReader + 'tx {
            TestApplicationPort {
                state: self.state.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopApplicationReader for TestApplicationPort {
        async fn list_all(
            &mut self,
        ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError>
        {
            self.state
                .applications
                .lock()
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })
                .map(|applications| {
                    applications
                        .iter()
                        .map(|application| view(&application.value))
                        .collect()
                })
        }

        async fn list_by_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError>
        {
            self.state
                .applications
                .lock()
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })
                .map(|applications| {
                    applications
                        .iter()
                        .filter(|application| application.value.applicant_user_id() == user_id)
                        .map(|application| view(&application.value))
                        .collect()
                })
        }
    }

    #[tokio::test]
    async fn should_list_applications_for_owner() -> Result<(), String> {
        let user_id = UserId::new();
        let application = application(user_id);
        let state = SharedState::with_application(application.clone());

        let result = ListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context(Principal::User(user_id)),
            ListPartnerShopApplicationsRequest { user_id },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(1, result.items.len());
        assert_eq!(application.id(), result.items[0].id);
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_allow_system_to_list_user_applications() {
        let user_id = UserId::new();
        let state = SharedState::with_application(application(user_id));

        let result = ListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::System),
            ListPartnerShopApplicationsRequest { user_id },
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_forbid_other_user() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = ListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::User(UserId::new())),
            ListPartnerShopApplicationsRequest { user_id },
        )
        .await;

        assert!(matches!(
            result,
            Err(ListPartnerShopApplicationsError::Forbidden)
        ));
    }

    fn view(application: &PartnerShopApplication) -> PartnerShopApplicationView {
        PartnerShopApplicationView {
            id: application.id(),
            applicant_user_id: application.applicant_user_id(),
            business_state: application.business_state(),
            payload: application.payload(),
            shop_id: application.shop_id(),
        }
    }

    fn application(user_id: UserId) -> PartnerShopApplication {
        PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: user_id,
            payload: PartnerShopApplicationPayload::Existing {
                shop_id: ShopId::new(),
            },
        })
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }
}
