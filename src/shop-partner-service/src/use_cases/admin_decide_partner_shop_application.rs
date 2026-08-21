use crate::admin_authorization::{AdminAuthorizationError, authorize_admin_actor};
use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory, UserPartnerShopMembershipRepository,
    UserPartnerShopMembershipRepositoryError, UserPartnerShopMembershipRepositoryFactory,
};
use application::{
    error::{BoxError, box_error},
    operation_context::{OperationAuthorizationError, OperationContext},
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::change_outcome::ChangeOutcome;
use notification_core::notification::{
    NotificationContent, PartnerApplicationDecision as NotificationPartnerApplicationDecision,
    PartnerApplicationNotificationSnapshot,
};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError, NotificationCreator,
    NotificationCreatorFactory,
};
pub use shop_partner_core::partner_shop_application::PartnerShopApplicationDecision;
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationTransitionError,
};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_service::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use user_service::ports::UserAdminReaderFactory;

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
    #[error("partner shop application is not decidable")]
    ApplicationNotDecidable,
    #[error("shop referenced by partner shop application not found")]
    ShopNotFound,
    #[error("shop cannot be published for partner application approval")]
    ShopNotPublishable,
    #[error("new partner shop application references a non-draft shop")]
    DraftShopNotDiscardable,
    #[error("concurrent partner shop application update")]
    ConcurrencyConflict,
    #[error("partner shop application decision notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
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

pub struct AdminDecidePartnerShopApplicationHandler<U, A, S, M, R, N> {
    unit_of_work: U,
    applications: A,
    shops: S,
    memberships: M,
    admin_reader: R,
    notifications: N,
}

impl<U, A, S, M, R, N> AdminDecidePartnerShopApplicationHandler<U, A, S, M, R, N> {
    pub fn new(
        unit_of_work: U,
        applications: A,
        shops: S,
        memberships: M,
        admin_reader: R,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            applications,
            shops,
            memberships,
            admin_reader,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, S, M, R, N> AdminDecidePartnerShopApplicationUseCase
    for AdminDecidePartnerShopApplicationHandler<U, A, S, M, R, N>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    S: ShopRepositoryFactory<U::Tx>,
    M: UserPartnerShopMembershipRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
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
        let mut shop = self
            .shops
            .in_transaction(&mut tx)
            .find_by_id(versioned.value.shop_id())
            .await?
            .ok_or(AdminDecidePartnerShopApplicationError::ShopNotFound)?;

        let notification = decision_notification(&versioned.value, &shop.shop, command.decision)?;

        if !versioned.value.has_applied_decision(command.decision) {
            match command.decision {
                PartnerShopApplicationDecision::Approve => {
                    versioned
                        .value
                        .approve()
                        .map_err(application_transition_error)?;

                    let partner_status_changed = shop.shop.change_partner_status(
                        shop_core::partner_status::ShopPartnerStatus::Partnered,
                    );
                    let lifecycle_changed = shop
                        .shop
                        .publish()
                        .map_err(|_| AdminDecidePartnerShopApplicationError::ShopNotPublishable)?;
                    if partner_status_changed == ChangeOutcome::Changed
                        || lifecycle_changed == ChangeOutcome::Changed
                    {
                        self.shops
                            .in_transaction(&mut tx)
                            .update(&shop.shop, shop.version)
                            .await?;
                    }

                    self.memberships
                        .in_transaction(&mut tx)
                        .grant(
                            versioned.value.applicant_user_id(),
                            versioned.value.shop_id(),
                        )
                        .await?;
                }
                PartnerShopApplicationDecision::Reject => {
                    versioned
                        .value
                        .reject()
                        .map_err(application_transition_error)?;

                    if versioned.value.is_new_shop_application() {
                        let discarded = shop.shop.discard().map_err(|_| {
                            AdminDecidePartnerShopApplicationError::DraftShopNotDiscardable
                        })?;
                        if discarded == ChangeOutcome::Changed {
                            self.shops
                                .in_transaction(&mut tx)
                                .update(&shop.shop, shop.version)
                                .await?;
                        }
                    }
                }
            }

            versioned = self
                .applications
                .in_transaction(&mut tx)
                .update(&versioned.value, versioned.version)
                .await?;
        }

        self.notifications
            .in_transaction(&mut tx)
            .create_many(&[notification])
            .await
            .map_err(|source: NotificationCreationError| {
                AdminDecidePartnerShopApplicationError::NotificationCreateFailed {
                    source: box_error(source),
                }
            })?;

        tx.commit()
            .await
            .map_err(|_| AdminDecidePartnerShopApplicationError::CommitTransactionFailed)?;

        tracing::info!(
            event = "partner_shop_application.decided",
            partner_shop_application_id = %versioned.value.id(),
            decision = ?command.decision,
            outcome = "success",
        );

        Ok(AdminDecidePartnerShopApplicationResult {
            application: versioned.value,
        })
    }
}

fn decision_notification(
    application: &PartnerShopApplication,
    shop: &shop_core::shop::Shop,
    decision: PartnerShopApplicationDecision,
) -> Result<NewNotification, AdminDecidePartnerShopApplicationError> {
    let decision = match decision {
        PartnerShopApplicationDecision::Approve => NotificationPartnerApplicationDecision::Approved,
        PartnerShopApplicationDecision::Reject => NotificationPartnerApplicationDecision::Rejected,
    };
    Ok(NewNotification {
        notification: notification_core::notification::Notification::new(
            Default::default(),
            application.applicant_user_id(),
            NotificationContent::PartnerApplication {
                partner_shop_application_id: application.id(),
                snapshot: PartnerApplicationNotificationSnapshot {
                    shop_name: shop.name().clone(),
                    image: shop.presentation().image.clone(),
                },
                decision,
            },
        ),
        external_delivery: ExternalDeliveryRequest::Requested,
    })
}

fn application_transition_error(
    _: PartnerShopApplicationTransitionError,
) -> AdminDecidePartnerShopApplicationError {
    AdminDecidePartnerShopApplicationError::ApplicationNotDecidable
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

impl From<UserPartnerShopMembershipRepositoryError> for AdminDecidePartnerShopApplicationError {
    fn from(error: UserPartnerShopMembershipRepositoryError) -> Self {
        match error {
            UserPartnerShopMembershipRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserPartnerShopMembershipRepositoryError::Internal { source } => {
                Self::Internal { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnerShopApplicationStorageVersion, UserPartnerShopMembershipRepository,
        VersionedPartnerShopApplication,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use notification_service::ports::notification_creator::NotificationCreationOutcome;
    use shop_core::{
        partner_status::ShopPartnerStatus,
        shop::{NewShop, Shop, ShopContact, ShopPresentation},
        shop_type::ShopType,
    };
    use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
    use shop_partner_core::partner_shop_application::{
        NewPartnerShopApplication, PartnerShopApplicationPayload,
    };
    use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
    use shop_service::ports::{ShopStorageVersion, StoredShop};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use user_core::user_id::UserId;
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
    struct SharedState {
        application: Arc<Mutex<Option<VersionedPartnerShopApplication>>>,
        shop: Arc<Mutex<Option<StoredShop>>>,
        memberships: Arc<Mutex<HashSet<(UserId, ShopId)>>>,
        application_updates: Arc<Mutex<usize>>,
        committed: Arc<Mutex<bool>>,
        notification_calls: Arc<Mutex<usize>>,
        fail_next_notification: Arc<Mutex<bool>>,
    }

    #[derive(Clone)]
    struct TestApplicationFactory(SharedState);
    struct TestApplicationRepository(SharedState);
    #[derive(Clone)]
    struct TestShopFactory(SharedState);
    struct TestShopRepository(SharedState);
    #[derive(Clone)]
    struct TestMembershipFactory(SharedState);
    struct TestMembershipRepository(SharedState);
    #[derive(Clone, Default)]
    struct TestAdminFactory;
    struct TestAdminReader;
    #[derive(Clone)]
    struct TestNotifier(SharedState);

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            *self
                .state
                .committed
                .lock()
                .map_err(|_| TransactionError::CommitFailed)? = true;
            Ok(())
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
            _: &'tx mut Tx,
        ) -> impl PartnerShopApplicationRepository + 'tx {
            TestApplicationRepository(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopApplicationRepository for TestApplicationRepository {
        async fn find_by_user_and_id(
            &mut self,
            _: UserId,
            _: PartnerShopApplicationId,
        ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
        {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            _: PartnerShopApplicationId,
        ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>
        {
            self.0
                .application
                .lock()
                .map(|application| application.clone())
                .map_err(|_| PartnerShopApplicationRepositoryError::Internal {
                    source: "application lock failed".into(),
                })
        }

        async fn insert(
            &mut self,
            _: &PartnerShopApplication,
        ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>
        {
            Err(PartnerShopApplicationRepositoryError::Internal {
                source: "unexpected insert".into(),
            })
        }

        async fn update(
            &mut self,
            application: &PartnerShopApplication,
            expected_version: PartnerShopApplicationStorageVersion,
        ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>
        {
            let mut stored = self.0.application.lock().map_err(|_| {
                PartnerShopApplicationRepositoryError::Internal {
                    source: "application lock failed".into(),
                }
            })?;
            let current = stored
                .as_ref()
                .ok_or(PartnerShopApplicationRepositoryError::ConcurrencyConflict)?;
            if current.version != expected_version {
                return Err(PartnerShopApplicationRepositoryError::ConcurrencyConflict);
            }
            let persisted =
                VersionedPartnerShopApplication::new(application.clone(), expected_version.next());
            *stored = Some(persisted.clone());
            *self.0.application_updates.lock().map_err(|_| {
                PartnerShopApplicationRepositoryError::Internal {
                    source: "update lock failed".into(),
                }
            })? += 1;
            Ok(persisted)
        }

        async fn delete(
            &mut self,
            _: PartnerShopApplicationId,
            _: PartnerShopApplicationStorageVersion,
        ) -> Result<(), PartnerShopApplicationRepositoryError> {
            Err(PartnerShopApplicationRepositoryError::Internal {
                source: "unexpected delete".into(),
            })
        }
    }

    impl<Tx> ShopRepositoryFactory<Tx> for TestShopFactory {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl ShopRepository + 'tx {
            TestShopRepository(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl ShopRepository for TestShopRepository {
        async fn find_by_id(
            &mut self,
            id: ShopId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            self.0
                .shop
                .lock()
                .map(|shop| shop.as_ref().filter(|shop| shop.shop.id() == id).cloned())
                .map_err(|_| ShopRepositoryError::Internal {
                    source: "shop lock failed".into(),
                })
        }

        async fn find_by_slug(
            &mut self,
            _: &ShopSlugId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, _: &Shop) -> Result<StoredShop, ShopRepositoryError> {
            Err(ShopRepositoryError::Internal {
                source: "unexpected insert".into(),
            })
        }

        async fn update(
            &mut self,
            shop: &Shop,
            expected_version: ShopStorageVersion,
        ) -> Result<StoredShop, ShopRepositoryError> {
            let mut stored = self
                .0
                .shop
                .lock()
                .map_err(|_| ShopRepositoryError::Internal {
                    source: "shop lock failed".into(),
                })?;
            let current = stored
                .as_ref()
                .ok_or(ShopRepositoryError::ConcurrencyConflict)?;
            if current.version != expected_version {
                return Err(ShopRepositoryError::ConcurrencyConflict);
            }
            let persisted = StoredShop {
                shop: shop.clone(),
                version: expected_version.next(),
                created: current.created,
                updated: current.updated,
            };
            *stored = Some(persisted.clone());
            Ok(persisted)
        }
    }

    impl<Tx> UserPartnerShopMembershipRepositoryFactory<Tx> for TestMembershipFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut Tx,
        ) -> impl UserPartnerShopMembershipRepository + 'tx {
            TestMembershipRepository(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl UserPartnerShopMembershipRepository for TestMembershipRepository {
        async fn grant(
            &mut self,
            user_id: UserId,
            shop_id: ShopId,
        ) -> Result<(), UserPartnerShopMembershipRepositoryError> {
            self.0
                .memberships
                .lock()
                .map_err(|_| UserPartnerShopMembershipRepositoryError::Internal {
                    source: "membership lock failed".into(),
                })?
                .insert((user_id, shop_id));
            Ok(())
        }
    }

    impl<Tx> UserAdminReaderFactory<Tx> for TestAdminFactory {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl UserAdminReader + 'tx {
            TestAdminReader
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for TestAdminReader {
        async fn find_admin_actor(
            &mut self,
            _: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(None)
        }
    }

    struct TestNotificationCreator(SharedState);

    impl NotificationCreatorFactory<TestTransaction> for TestNotifier {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl NotificationCreator + 'tx {
            TestNotificationCreator(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for TestNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            *self.0.notification_calls.lock().map_err(|_| {
                NotificationCreationError::CreateFailed {
                    source: "notification lock failed".into(),
                }
            })? += 1;
            let mut fail = self.0.fail_next_notification.lock().map_err(|_| {
                NotificationCreationError::CreateFailed {
                    source: "notification failure lock failed".into(),
                }
            })?;
            if *fail {
                *fail = false;
                return Err(NotificationCreationError::CreateFailed {
                    source: "notification failed".into(),
                });
            }
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect())
        }
    }

    #[tokio::test]
    async fn should_approve_new_shop_and_grant_membership() {
        let state = state_for(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);

        let result = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Approve,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            PartnerShopApplicationState::Approved,
            application(&state).business_state()
        );
        assert_eq!(ShopPartnerStatus::Partnered, shop(&state).partner_status());
        assert_eq!(
            shop_core::lifecycle::ShopLifecycle::Published,
            shop(&state).lifecycle()
        );
        assert_eq!(
            1,
            state
                .memberships
                .lock()
                .map(|items| items.len())
                .unwrap_or_default()
        );
        assert!(
            state
                .committed
                .lock()
                .map(|committed| *committed)
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn should_approve_existing_shop_and_grant_membership() {
        let state = state_for(PartnerShopApplicationPayload::Existing {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);

        let result = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Approve,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            PartnerShopApplicationState::Approved,
            application(&state).business_state()
        );
        assert_eq!(ShopPartnerStatus::Partnered, shop(&state).partner_status());
        assert_eq!(
            1,
            state
                .memberships
                .lock()
                .map(|items| items.len())
                .unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn should_reject_new_shop_and_discard_its_draft() {
        let state = state_for(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);

        let result = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Reject,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            PartnerShopApplicationState::Rejected,
            application(&state).business_state()
        );
        assert_eq!(
            shop_core::lifecycle::ShopLifecycle::Discarded,
            shop(&state).lifecycle()
        );
        assert_eq!(
            0,
            state
                .memberships
                .lock()
                .map(|items| items.len())
                .unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn should_reject_existing_shop_without_discarding_it() {
        let state = state_for(PartnerShopApplicationPayload::Existing {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);

        let result = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Reject,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            PartnerShopApplicationState::Rejected,
            application(&state).business_state()
        );
        assert_eq!(
            shop_core::lifecycle::ShopLifecycle::Drafted,
            shop(&state).lifecycle()
        );
    }

    #[tokio::test]
    async fn should_replay_same_decision_without_repeating_authoritative_writes() {
        let state = state_for(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);

        assert!(
            decide(
                &state,
                application_id,
                PartnerShopApplicationDecision::Approve
            )
            .await
            .is_ok()
        );
        assert!(
            decide(
                &state,
                application_id,
                PartnerShopApplicationDecision::Approve
            )
            .await
            .is_ok()
        );

        assert_eq!(
            1,
            state
                .application_updates
                .lock()
                .map(|value| *value)
                .unwrap_or_default()
        );
        assert_eq!(
            2,
            state
                .notification_calls
                .lock()
                .map(|value| *value)
                .unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn should_reject_decision_after_a_different_terminal_state() {
        let state = state_for(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        let application_id = application_id(&state);
        assert!(
            decide(
                &state,
                application_id,
                PartnerShopApplicationDecision::Reject
            )
            .await
            .is_ok()
        );

        let result = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Approve,
        )
        .await;

        assert!(matches!(
            result,
            Err(AdminDecidePartnerShopApplicationError::ApplicationNotDecidable)
        ));
    }

    #[tokio::test]
    async fn should_report_notification_creation_failure() {
        let state = state_for(PartnerShopApplicationPayload::New {
            shop_id: ShopId::new(),
        });
        *state
            .fail_next_notification
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let application_id = application_id(&state);

        let first = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Approve,
        )
        .await;
        let retry = decide(
            &state,
            application_id,
            PartnerShopApplicationDecision::Approve,
        )
        .await;

        assert!(matches!(
            first,
            Err(AdminDecidePartnerShopApplicationError::NotificationCreateFailed { .. })
        ));
        assert!(retry.is_ok());
        assert_eq!(
            PartnerShopApplicationState::Approved,
            application(&state).business_state()
        );
        assert_eq!(
            2,
            state
                .notification_calls
                .lock()
                .map(|value| *value)
                .unwrap_or_default()
        );
    }

    fn state_for(payload: PartnerShopApplicationPayload) -> SharedState {
        let state = SharedState::default();
        let shop_id = match payload {
            PartnerShopApplicationPayload::Existing { shop_id }
            | PartnerShopApplicationPayload::New { shop_id } => shop_id,
        };
        let applicant_user_id = UserId::new();
        let mut application = PartnerShopApplication::create(NewPartnerShopApplication {
            id: PartnerShopApplicationId::new(),
            applicant_user_id,
            payload,
        });
        let _ = application.mark_in_review();
        *state
            .application
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(VersionedPartnerShopApplication::new(
                application,
                PartnerShopApplicationStorageVersion::INITIAL,
            ));
        *state.shop.lock().unwrap_or_else(|error| error.into_inner()) = Some(StoredShop {
            shop: Shop::create(NewShop {
                id: shop_id,
                name: ShopName::from("Partner shop"),
                shop_type: ShopType::CommercialDealer,
                domains: HashSet::new(),
                shopify: None,
                woocommerce: None,
                presentation: ShopPresentation::default(),
                address: None,
                contact: ShopContact::default(),
                partner_status: ShopPartnerStatus::Scraped,
                affiliate_configuration: None,
            }),
            version: ShopStorageVersion::INITIAL,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        });
        state
    }

    async fn decide(
        state: &SharedState,
        application_id: PartnerShopApplicationId,
        decision: PartnerShopApplicationDecision,
    ) -> Result<AdminDecidePartnerShopApplicationResult, AdminDecidePartnerShopApplicationError>
    {
        AdminDecidePartnerShopApplicationHandler::new(
            TestUnitOfWork {
                state: state.clone(),
            },
            TestApplicationFactory(state.clone()),
            TestShopFactory(state.clone()),
            TestMembershipFactory(state.clone()),
            TestAdminFactory,
            TestNotifier(state.clone()),
        )
        .execute(
            &context(),
            AdminDecidePartnerShopApplicationCommand {
                application_id,
                decision,
            },
        )
        .await
    }

    fn application_id(state: &SharedState) -> PartnerShopApplicationId {
        application(state).id()
    }

    fn application(state: &SharedState) -> PartnerShopApplication {
        state
            .application
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|application| application.value.clone())
            .unwrap_or_else(|| panic!("application missing"))
    }

    fn shop(state: &SharedState) -> Shop {
        state
            .shop
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|shop| shop.shop.clone())
            .unwrap_or_else(|| panic!("shop missing"))
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }
}
