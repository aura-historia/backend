use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::user_id::UserId;
use shop_partner_core::partner_shop_application::PartnerShopApplication;

#[derive(Debug, Clone, PartialEq)]
pub struct GetPartnerShopApplicationRequest {
    pub user_id: UserId,
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetPartnerShopApplicationResult {
    pub application: PartnerShopApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum GetPartnerShopApplicationError {
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
pub trait GetPartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnerShopApplicationRequest,
    ) -> Result<GetPartnerShopApplicationResult, GetPartnerShopApplicationError>;
}

pub struct GetPartnerShopApplicationHandler<U, A> {
    unit_of_work: U,
    applications: A,
}

impl<U, A> GetPartnerShopApplicationHandler<U, A> {
    pub fn new(unit_of_work: U, applications: A) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}

#[async_trait::async_trait]
impl<U, A> GetPartnerShopApplicationUseCase for GetPartnerShopApplicationHandler<U, A>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "get_partner_shop_application", skip_all, fields(user_id = %request.user_id, partner_shop_application_id = %request.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnerShopApplicationRequest,
    ) -> Result<GetPartnerShopApplicationResult, GetPartnerShopApplicationError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
            .user(&request.user_id)
            .service_or_system()
            .authorize::<GetPartnerShopApplicationError>()?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetPartnerShopApplicationError::BeginTransactionFailed)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(request.user_id, request.application_id)
            .await?
            .ok_or(GetPartnerShopApplicationError::NotFound)?
            .value;
        tx.commit()
            .await
            .map_err(|_| GetPartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(GetPartnerShopApplicationResult { application })
    }
}

impl From<OperationAuthorizationError> for GetPartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for GetPartnerShopApplicationError {
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
    use application::transaction::TransactionError;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId};
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplicationPayload,
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
            _id: PartnerShopApplicationId,
            _expected_version: PartnerShopApplicationStorageVersion,
        ) -> Result<(), PartnerShopApplicationRepositoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_get_application_for_owner() -> Result<(), String> {
        let user_id = UserId::new();
        let application = application(user_id);
        let application_id = application.id();
        let state = SharedState::with_application(application.clone());
        let result = GetPartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context(Principal::User(user_id)),
            GetPartnerShopApplicationRequest {
                user_id,
                application_id,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(application, result.application);
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_application_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();
        let result = GetPartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::User(user_id)),
            GetPartnerShopApplicationRequest {
                user_id,
                application_id: PartnerShopApplicationId::new(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(GetPartnerShopApplicationError::NotFound)
        ));
    }

    #[tokio::test]
    async fn should_forbid_other_user() {
        let user_id = UserId::new();
        let state = SharedState::default();
        let result = GetPartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
        )
        .execute(
            &context(Principal::User(UserId::new())),
            GetPartnerShopApplicationRequest {
                user_id,
                application_id: PartnerShopApplicationId::new(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(GetPartnerShopApplicationError::Forbidden)
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
