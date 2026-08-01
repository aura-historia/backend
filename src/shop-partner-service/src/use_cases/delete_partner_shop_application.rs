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
pub struct DeletePartnerShopApplicationCommand {
    pub user_id: UserId,
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeletePartnerShopApplicationError {
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
pub trait DeletePartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeletePartnerShopApplicationCommand,
    ) -> Result<(), DeletePartnerShopApplicationError>;
}

pub struct DeletePartnerShopApplicationHandler<U, A> {
    unit_of_work: U,
    applications: A,
}

impl<U, A> DeletePartnerShopApplicationHandler<U, A> {
    pub fn new(unit_of_work: U, applications: A) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}

#[async_trait::async_trait]
impl<U, A> DeletePartnerShopApplicationUseCase for DeletePartnerShopApplicationHandler<U, A>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "delete_partner_shop_application", skip_all, fields(user_id = %command.user_id, partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeletePartnerShopApplicationCommand,
    ) -> Result<(), DeletePartnerShopApplicationError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
            .user(&command.user_id)
            .service_or_system()
            .authorize::<DeletePartnerShopApplicationError>()?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeletePartnerShopApplicationError::BeginTransactionFailed)?;
        let versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(command.user_id, command.application_id)
            .await?
            .ok_or(DeletePartnerShopApplicationError::NotFound)?;
        self.applications
            .in_transaction(&mut tx)
            .delete(versioned.value.id(), versioned.version)
            .await?;
        tx.commit()
            .await
            .map_err(|_| DeletePartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(())
    }
}

impl From<OperationAuthorizationError> for DeletePartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for DeletePartnerShopApplicationError {
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
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::transaction::TransactionError;
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
        fn committed(&self) -> bool {
            self.committed.lock().map(|value| *value).unwrap_or(false)
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
            Ok(VersionedPartnerShopApplication::new(
                application.clone(),
                expected_version.next(),
            ))
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
    async fn should_delete_application_for_owner() -> Result<(), String> {
        let user_id = UserId::new();
        let application = application(user_id);
        let application_id = application.id();
        let state = SharedState::with_application(application);
        DeletePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context(Principal::User(user_id)),
            DeletePartnerShopApplicationCommand {
                user_id,
                application_id,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(1, state.deleted());
        assert!(state.committed());
        Ok(())
    }
    #[tokio::test]
    async fn should_return_not_found_when_application_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();
        let result = DeletePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::User(user_id)),
            DeletePartnerShopApplicationCommand {
                user_id,
                application_id: PartnerShopApplicationId::new(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(DeletePartnerShopApplicationError::NotFound)
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
