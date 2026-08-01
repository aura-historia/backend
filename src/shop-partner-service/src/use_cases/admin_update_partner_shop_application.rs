use crate::admin_authorization::{AdminAuthorizationError, authorize_admin_actor};
use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use common::error::boxed::BoxError;
use common::operation_context::{OperationAuthorizationError, OperationContext};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::transaction::{Transaction, UnitOfWork};
use shop_partner_core::partner_shop_application::PartnerShopApplication;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq)]
pub struct AdminGetPartnerShopApplicationForUpdateRequest {
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminMarkPartnerShopApplicationInReviewCommand {
    pub application_id: PartnerShopApplicationId,
    pub task_token: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminUpdatePartnerShopApplicationResult {
    pub application: PartnerShopApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminUpdatePartnerShopApplicationError {
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
pub trait AdminUpdatePartnerShopApplicationUseCase: Send + Sync {
    async fn mark_in_review(
        &self,
        context: &OperationContext,
        command: AdminMarkPartnerShopApplicationInReviewCommand,
    ) -> Result<AdminUpdatePartnerShopApplicationResult, AdminUpdatePartnerShopApplicationError>;
}

pub struct AdminUpdatePartnerShopApplicationHandler<U, A, R> {
    unit_of_work: U,
    applications: A,
    admin_reader: R,
}

impl<U, A, R> AdminUpdatePartnerShopApplicationHandler<U, A, R> {
    pub fn new(unit_of_work: U, applications: A, admin_reader: R) -> Self {
        Self {
            unit_of_work,
            applications,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, R> AdminUpdatePartnerShopApplicationUseCase
    for AdminUpdatePartnerShopApplicationHandler<U, A, R>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "admin_mark_partner_shop_application_in_review", skip_all, fields(partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn mark_in_review(
        &self,
        context: &OperationContext,
        command: AdminMarkPartnerShopApplicationInReviewCommand,
    ) -> Result<AdminUpdatePartnerShopApplicationResult, AdminUpdatePartnerShopApplicationError>
    {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| AdminUpdatePartnerShopApplicationError::BeginTransactionFailed)?;
        authorize_admin_actor(context, &mut tx, &self.admin_reader).await?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(command.application_id)
            .await?
            .ok_or(AdminUpdatePartnerShopApplicationError::NotFound)?;
        versioned.value.mark_in_review(command.task_token);
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| AdminUpdatePartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(AdminUpdatePartnerShopApplicationResult { application })
    }
}

impl From<AdminAuthorizationError> for AdminUpdatePartnerShopApplicationError {
    fn from(error: AdminAuthorizationError) -> Self {
        match error {
            AdminAuthorizationError::Forbidden
            | AdminAuthorizationError::Operation(
                OperationAuthorizationError::AuthenticationRequired(_),
            )
            | AdminAuthorizationError::Operation(OperationAuthorizationError::Forbidden)
            | AdminAuthorizationError::Operation(
                OperationAuthorizationError::InsufficientCapability { .. },
            ) => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<OperationAuthorizationError> for AdminUpdatePartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for AdminUpdatePartnerShopApplicationError {
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
    use common::{
        partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId, user_id::UserId,
    };
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplicationPayload,
    };
    use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
    use std::sync::{Arc, Mutex};
    use user_service::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};
    use user_service::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};

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
    }
    struct TestApplicationPort {
        state: SharedState,
    }
    #[derive(Clone, Default)]
    struct TestAdminFactory;
    struct TestAdminReader;
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
        fn updated(&self) -> usize {
            self.updated.lock().map(|value| *value).unwrap_or(0)
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
    impl<Tx> UserAdminReaderFactory<Tx> for TestAdminFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl UserAdminReader + 'tx {
            TestAdminReader
        }
    }
    #[async_trait::async_trait]
    impl UserAdminReader for TestAdminReader {
        async fn find_admin_view(
            &mut self,
            _request: &GetUserRequest,
        ) -> Result<Option<UserDetailsView>, UserAdminReadError> {
            Ok(None)
        }
    }
    #[async_trait::async_trait]
    impl PartnerShopApplicationRepository for TestApplicationPort {
        async fn find_by_user_and_id(
            &mut self,
            _user_id: UserId,
            _id: PartnerShopApplicationId,
        ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
        {
            Ok(None)
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
            let mut applications = self.state.applications.lock().map_err(|_| {
                PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                }
            })?;
            let Some(existing) = applications
                .iter_mut()
                .find(|existing| existing.value.id() == application.id())
            else {
                return Err(PartnerShopApplicationRepositoryError::Internal {
                    source: "missing app".into(),
                });
            };
            existing.value = application.clone();
            existing.version = expected_version.next();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "lock failed".into(),
                })?;
            Ok(existing.clone())
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
    async fn should_mark_application_in_review_for_system() -> Result<(), String> {
        let application = application(UserId::new());
        let application_id = application.id();
        let state = SharedState::with_application(application);
        let result = AdminUpdatePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
            TestAdminFactory,
        )
        .mark_in_review(
            &context(Principal::System),
            AdminMarkPartnerShopApplicationInReviewCommand {
                application_id,
                task_token: "token".to_owned(),
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(
            PartnerShopApplicationState::InReview,
            result.application.business_state()
        );
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_application_missing() {
        let state = SharedState::default();
        let result = AdminUpdatePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
            TestAdminFactory,
        )
        .mark_in_review(
            &context(Principal::System),
            AdminMarkPartnerShopApplicationInReviewCommand {
                application_id: PartnerShopApplicationId::new(),
                task_token: "token".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(AdminUpdatePartnerShopApplicationError::NotFound)
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
