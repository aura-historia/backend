use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnerShopApplicationCommand {
    pub user_id: UserId,
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, thiserror::Error)]
pub enum WithdrawPartnerShopApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partner shop application not found")]
    NotFound,
    #[error("concurrent partner shop application update")]
    ConcurrencyConflict,
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
pub trait WithdrawPartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnerShopApplicationCommand,
    ) -> Result<(), WithdrawPartnerShopApplicationError>;
}

pub struct WithdrawPartnerShopApplicationHandler<U, A> {
    unit_of_work: U,
    applications: A,
}

impl<U, A> WithdrawPartnerShopApplicationHandler<U, A> {
    pub fn new(unit_of_work: U, applications: A) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}

#[async_trait::async_trait]
impl<U, A> WithdrawPartnerShopApplicationUseCase for WithdrawPartnerShopApplicationHandler<U, A>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "withdraw_partner_shop_application", skip_all, fields(user_id = %command.user_id, partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnerShopApplicationCommand,
    ) -> Result<(), WithdrawPartnerShopApplicationError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
            .user(&command.user_id)
            .service_or_system()
            .authorize::<WithdrawPartnerShopApplicationError>()?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawPartnerShopApplicationError::BeginTransactionFailed)?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(command.user_id, command.application_id)
            .await?
            .ok_or(WithdrawPartnerShopApplicationError::NotFound)?;
        versioned.value.withdraw();
        self.applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?;
        tx.commit()
            .await
            .map_err(|_| WithdrawPartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(())
    }
}

impl From<OperationAuthorizationError> for WithdrawPartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for WithdrawPartnerShopApplicationError {
    fn from(error: PartnerShopApplicationRepositoryError) -> Self {
        match error {
            PartnerShopApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
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
    use crate::ports::{PartnerShopApplicationStorageVersion, VersionedPartnerShopApplication};
    use common::execution_state::domain::ExecutionState;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::transaction::TransactionError;
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId};
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
    };
    use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
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
        updated: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<usize>>,
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
        fn application(
            &self,
            id: PartnerShopApplicationId,
        ) -> Result<Option<PartnerShopApplication>, String> {
            self.applications
                .lock()
                .map_err(|_| "lock failed".to_owned())
                .map(|applications| {
                    applications
                        .iter()
                        .find(|application| application.value.id() == id)
                        .map(|application| application.value.clone())
                })
        }
        fn committed(&self) -> bool {
            self.committed.lock().map(|value| *value).unwrap_or(false)
        }
        fn updated(&self) -> usize {
            self.updated.lock().map(|value| *value).unwrap_or(0)
        }
        fn deleted(&self) -> usize {
            self.deleted.lock().map(|value| *value).unwrap_or(0)
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
    impl<Tx> PartnerShopApplicationRepositoryFactory<Tx> for TestApplicationFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl PartnerShopApplicationRepository + 'tx {
            TestApplicationPort {
                state: self.state.clone(),
            }
        }
    }
    #[async_trait::async_trait]
    impl PartnerShopApplicationRepository for TestApplicationPort {
        async fn find_by_user_and_id(
            &mut self,
            user_id: UserId,
            id: PartnerShopApplicationId,
        ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
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
                        .find(|application| {
                            application.value.applicant_user_id() == user_id
                                && application.value.id() == id
                        })
                        .cloned()
                })
        }
        async fn find_by_id(
            &mut self,
            id: PartnerShopApplicationId,
        ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
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
                        .find(|application| application.value.id() == id)
                        .cloned()
                })
        }
        async fn insert(
            &mut self,
            application: &PartnerShopApplication,
        ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>
        {
            Ok(VersionedPartnerShopApplication::new(
                application.clone(),
                PartnerShopApplicationStorageVersion::INITIAL,
            ))
        }
        async fn update(
            &mut self,
            application: &PartnerShopApplication,
            expected_version: PartnerShopApplicationStorageVersion,
        ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>
        {
            let persisted =
                VersionedPartnerShopApplication::new(application.clone(), expected_version.next());
            let mut applications = self.state.applications.lock().map_err(|_| {
                PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                }
            })?;
            let existing = applications
                .iter_mut()
                .find(|existing| existing.value.id() == application.id())
                .ok_or(PartnerShopApplicationRepositoryError::ConcurrencyConflict)?;
            *existing = persisted.clone();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })?;
            Ok(persisted)
        }
        async fn delete(
            &mut self,
            id: PartnerShopApplicationId,
            _expected_version: PartnerShopApplicationStorageVersion,
        ) -> Result<(), PartnerShopApplicationRepositoryError> {
            self.state
                .applications
                .lock()
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })?
                .retain(|application| application.value.id() != id);
            self.state
                .deleted
                .lock()
                .map(|mut deleted| *deleted += 1)
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })
        }
    }

    #[tokio::test]
    async fn should_withdraw_application_for_owner() -> Result<(), String> {
        let user_id = UserId::new();
        let application = application(user_id);
        let application_id = application.id();
        let state = SharedState::with_application(application);
        WithdrawPartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context(Principal::User(user_id)),
            WithdrawPartnerShopApplicationCommand {
                user_id,
                application_id,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        let application = state
            .application(application_id)?
            .ok_or_else(|| "application missing".to_owned())?;
        assert_eq!(
            PartnerShopApplicationState::Withdrawn,
            application.business_state()
        );
        assert_eq!(ExecutionState::Completed, application.execution_state());
        assert_eq!(1, state.updated());
        assert_eq!(0, state.deleted());
        assert!(state.committed());
        Ok(())
    }
    #[tokio::test]
    async fn should_return_not_found_when_application_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();
        let result = WithdrawPartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::User(user_id)),
            WithdrawPartnerShopApplicationCommand {
                user_id,
                application_id: PartnerShopApplicationId::new(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(WithdrawPartnerShopApplicationError::NotFound)
        ));
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
