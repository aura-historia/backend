use crate::admin_authorization::{AdminAuthorizationError, authorize_admin_actor};
use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use common::change_outcome::ChangeOutcome;
use common::error::boxed::{BoxError, static_error};
use common::operation_context::{OperationAuthorizationError, OperationContext};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::transaction::{Transaction, UnitOfWork};
use shop_partner_core::partner_shop_application::PartnerShopApplication;
use shop_service::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminDecidePartnerShopApplicationCommand {
    pub application_id: PartnerShopApplicationId,
    pub decision: PartnerShopApplicationDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminDecidePartnerShopApplicationResult {
    pub application: PartnerShopApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminDecidePartnerShopApplicationError {
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
pub trait AdminDecidePartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminDecidePartnerShopApplicationCommand,
    ) -> Result<AdminDecidePartnerShopApplicationResult, AdminDecidePartnerShopApplicationError>;
}

pub struct AdminDecidePartnerShopApplicationHandler<U, A, S, R> {
    unit_of_work: U,
    applications: A,
    shops: S,
    admin_reader: R,
}
impl<U, A, S, R> AdminDecidePartnerShopApplicationHandler<U, A, S, R> {
    pub fn new(unit_of_work: U, applications: A, shops: S, admin_reader: R) -> Self {
        Self {
            unit_of_work,
            applications,
            shops,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, S, R> AdminDecidePartnerShopApplicationUseCase
    for AdminDecidePartnerShopApplicationHandler<U, A, S, R>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    S: ShopRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "admin_decide_partner_shop_application", skip_all, fields(partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminDecidePartnerShopApplicationCommand,
    ) -> Result<AdminDecidePartnerShopApplicationResult, AdminDecidePartnerShopApplicationError>
    {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| AdminDecidePartnerShopApplicationError::BeginTransactionFailed)?;
        authorize_admin_actor(context, &mut tx, &self.admin_reader).await?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(command.application_id)
            .await?
            .ok_or(AdminDecidePartnerShopApplicationError::NotFound)?;
        match command.decision {
            PartnerShopApplicationDecision::Approve => {
                let mut shop = self
                    .shops
                    .in_transaction(&mut tx)
                    .find_by_id(versioned.value.shop_id())
                    .await?
                    .ok_or(
                        AdminDecidePartnerShopApplicationError::InvalidPersistedState {
                            source: static_error(
                                "partner shop application references a missing shop",
                            ),
                        },
                    )?;
                if shop.shop.publish() == ChangeOutcome::Changed {
                    self.shops
                        .in_transaction(&mut tx)
                        .update(&shop.shop, shop.version)
                        .await?;
                }
                versioned.value.approve();
            }
            PartnerShopApplicationDecision::Reject => versioned.value.reject(),
        }
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| AdminDecidePartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(AdminDecidePartnerShopApplicationResult { application })
    }
}
impl From<AdminAuthorizationError> for AdminDecidePartnerShopApplicationError {
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
impl From<OperationAuthorizationError> for AdminDecidePartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<ShopRepositoryError> for AdminDecidePartnerShopApplicationError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::SlugConflict { source }
            | ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for AdminDecidePartnerShopApplicationError {
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
    use shop_core::{
        partner_status::ShopPartnerStatus,
        shop::{NewShop, Shop, ShopContact, ShopPresentation},
        shop_type::ShopType,
    };
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplicationPayload,
    };
    use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
    use shop_service::ports::{
        ShopRepository, ShopRepositoryFactory, ShopStorageVersion, StoredShop,
    };
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };
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
        updated: Arc<Mutex<usize>>,
        shop: Arc<Mutex<Option<StoredShop>>>,
    }
    struct TestApplicationPort {
        state: SharedState,
    }
    #[derive(Clone, Default)]
    struct TestShopFactory {
        state: SharedState,
    }
    struct TestShopPort {
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
        fn with_shop(&self, shop: Shop) {
            if let Ok(mut stored) = self.shop.lock() {
                *stored = Some(StoredShop {
                    shop,
                    version: ShopStorageVersion::INITIAL,
                    created: time::OffsetDateTime::UNIX_EPOCH,
                    updated: time::OffsetDateTime::UNIX_EPOCH,
                });
            }
        }
        fn shop_lifecycle(&self) -> Option<shop_core::lifecycle::ShopLifecycle> {
            self.shop
                .lock()
                .ok()
                .and_then(|stored| stored.as_ref().map(|shop| shop.shop.lifecycle()))
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
    impl<Tx> ShopRepositoryFactory<Tx> for TestShopFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl ShopRepository + 'tx {
            TestShopPort {
                state: self.state.clone(),
            }
        }
    }
    #[async_trait::async_trait]
    impl ShopRepository for TestShopPort {
        async fn find_by_id(
            &mut self,
            id: ShopId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            self.state
                .shop
                .lock()
                .map_err(|_| ShopRepositoryError::Internal {
                    source: "lock failed".into(),
                })
                .map(|stored| stored.as_ref().filter(|shop| shop.shop.id() == id).cloned())
        }
        async fn find_by_slug(
            &mut self,
            _slug_id: &common::shop_slug_id::ShopSlugId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            Ok(None)
        }
        async fn insert(&mut self, shop: &Shop) -> Result<StoredShop, ShopRepositoryError> {
            Ok(StoredShop {
                shop: shop.clone(),
                version: ShopStorageVersion::INITIAL,
                created: time::OffsetDateTime::UNIX_EPOCH,
                updated: time::OffsetDateTime::UNIX_EPOCH,
            })
        }
        async fn update(
            &mut self,
            shop: &Shop,
            expected_version: ShopStorageVersion,
        ) -> Result<StoredShop, ShopRepositoryError> {
            let mut stored = self
                .state
                .shop
                .lock()
                .map_err(|_| ShopRepositoryError::Internal {
                    source: "lock failed".into(),
                })?;
            let Some(existing) = stored
                .as_mut()
                .filter(|existing| existing.shop.id() == shop.id())
            else {
                return Err(ShopRepositoryError::ConcurrencyConflict);
            };
            existing.shop = shop.clone();
            existing.version = expected_version.next();
            Ok(existing.clone())
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
    async fn should_approve_application_and_publish_linked_shop_for_system() -> Result<(), String> {
        let shop = shop();
        let application = application_for(UserId::new(), shop.id());
        let application_id = application.id();
        let state = SharedState::with_application(application);
        state.with_shop(shop);
        let result = AdminDecidePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
            TestShopFactory {
                state: state.clone(),
            },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::System),
            AdminDecidePartnerShopApplicationCommand {
                application_id,
                decision: PartnerShopApplicationDecision::Approve,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(
            PartnerShopApplicationState::Approved,
            result.application.business_state()
        );
        assert_eq!(1, state.updated());
        assert_eq!(
            Some(shop_core::lifecycle::ShopLifecycle::Published),
            state.shop_lifecycle()
        );
        assert!(state.committed());
        Ok(())
    }
    #[tokio::test]
    async fn should_reject_application_without_publishing_linked_shop_for_system()
    -> Result<(), String> {
        let shop = shop();
        let application = application_for(UserId::new(), shop.id());
        let application_id = application.id();
        let state = SharedState::with_application(application);
        state.with_shop(shop);
        let result = AdminDecidePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
            TestShopFactory {
                state: state.clone(),
            },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::System),
            AdminDecidePartnerShopApplicationCommand {
                application_id,
                decision: PartnerShopApplicationDecision::Reject,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(
            PartnerShopApplicationState::Rejected,
            result.application.business_state()
        );
        assert_eq!(
            Some(shop_core::lifecycle::ShopLifecycle::Drafted),
            state.shop_lifecycle()
        );
        Ok(())
    }
    #[tokio::test]
    async fn should_forbid_plain_user() {
        let state = SharedState::default();
        let result = AdminDecidePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory {
                state: state.clone(),
            },
            TestShopFactory { state },
            TestAdminFactory,
        )
        .execute(
            &context(Principal::User(UserId::new())),
            AdminDecidePartnerShopApplicationCommand {
                application_id: PartnerShopApplicationId::new(),
                decision: PartnerShopApplicationDecision::Approve,
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(AdminDecidePartnerShopApplicationError::Forbidden)
        ));
    }
    fn application_for(user_id: UserId, shop_id: ShopId) -> PartnerShopApplication {
        PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id: user_id,
            payload: PartnerShopApplicationPayload::New { shop_id },
        })
    }
    fn shop() -> Shop {
        Shop::create(NewShop {
            id: ShopId::new(),
            name: "Partner application shop".into(),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify: None,
            woocommerce: None,
            presentation: ShopPresentation::default(),
            address: None,
            contact: ShopContact::default(),
            partner_status: ShopPartnerStatus::Partnered,
            affiliate_configuration: None,
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
