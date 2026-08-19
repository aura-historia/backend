use crate::admin_authorization::{AdminAuthorizationError, authorize_admin_actor};
use crate::ports::{
    PartnerShopApplicationReader, PartnerShopApplicationReaderFactory,
    PartnerShopApplicationRepositoryError, PartnerShopApplicationView,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{OperationAuthorizationError, OperationContext};
use common::user_id::UserId;

use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdminListPartnerShopApplicationsRequest {
    pub user_id: Option<UserId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminListPartnerShopApplicationsResult {
    pub items: Vec<PartnerShopApplicationView>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminListPartnerShopApplicationsError {
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
pub trait AdminListPartnerShopApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminListPartnerShopApplicationsRequest,
    ) -> Result<AdminListPartnerShopApplicationsResult, AdminListPartnerShopApplicationsError>;
}

pub struct AdminListPartnerShopApplicationsHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admin_reader: A,
}
impl<U, R, A> AdminListPartnerShopApplicationsHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> AdminListPartnerShopApplicationsUseCase
    for AdminListPartnerShopApplicationsHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartnerShopApplicationReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "admin_list_partner_shop_applications", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminListPartnerShopApplicationsRequest,
    ) -> Result<AdminListPartnerShopApplicationsResult, AdminListPartnerShopApplicationsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| AdminListPartnerShopApplicationsError::BeginTransactionFailed)?;
        authorize_admin_actor(context, &mut tx, &self.admin_reader).await?;
        let items = match request.user_id {
            Some(user_id) => {
                self.reader
                    .in_transaction(&mut tx)
                    .list_by_user(user_id)
                    .await?
            }
            None => self.reader.in_transaction(&mut tx).list_all().await?,
        };
        tx.commit()
            .await
            .map_err(|_| AdminListPartnerShopApplicationsError::CommitTransactionFailed)?;
        Ok(AdminListPartnerShopApplicationsResult { items })
    }
}
impl From<AdminAuthorizationError> for AdminListPartnerShopApplicationsError {
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
impl From<OperationAuthorizationError> for AdminListPartnerShopApplicationsError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<PartnerShopApplicationRepositoryError> for AdminListPartnerShopApplicationsError {
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
    use crate::ports::{PartnerShopApplicationStorageVersion, VersionedPartnerShopApplication};
    use application::transaction::TransactionError;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_id::ShopId};
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplication, PartnerShopApplicationPayload,
    };
    use std::sync::{Arc, Mutex};
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };
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
    impl<Tx> UserAdminReaderFactory<Tx> for TestAdminFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl UserAdminReader + 'tx {
            TestAdminReader
        }
    }
    #[async_trait::async_trait]
    impl UserAdminReader for TestAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(None)
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
    async fn should_list_all_applications_for_system() -> Result<(), String> {
        let user_id = UserId::new();
        let state = SharedState::with_application(application(user_id));
        let result = AdminListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::System),
            AdminListPartnerShopApplicationsRequest::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(1, result.items.len());
        assert!(state.committed());
        Ok(())
    }
    #[tokio::test]
    async fn should_filter_applications_by_user_for_system() -> Result<(), String> {
        let user_id = UserId::new();
        let other_user_id = UserId::new();
        let state = SharedState::with_application(application(user_id));
        state.push(application(other_user_id));
        let result = AdminListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::System),
            AdminListPartnerShopApplicationsRequest {
                user_id: Some(user_id),
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(1, result.items.len());
        assert_eq!(user_id, result.items[0].applicant_user_id);
        Ok(())
    }
    #[tokio::test]
    async fn should_forbid_plain_user() {
        let state = SharedState::default();
        let result = AdminListPartnerShopApplicationsHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory { state },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::User(UserId::new())),
            AdminListPartnerShopApplicationsRequest::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(AdminListPartnerShopApplicationsError::Forbidden)
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
