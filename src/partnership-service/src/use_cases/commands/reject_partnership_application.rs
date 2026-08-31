use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::{BoxError, box_error, static_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use notification_core::{
    notification::{
        Notification, NotificationContent, PartnershipApplicationDecision,
        PartnershipApplicationNotificationSnapshot,
    },
    notification_id::NotificationId,
};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError, NotificationCreator,
    NotificationCreatorFactory,
};
use partnership_core::{
    partnership_application::{PartnershipApplication, PartnershipProposal},
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
};
use party_core::party_name::PartyName;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq)]
pub struct RejectPartnershipApplicationCommand {
    pub application_id: PartnershipApplicationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RejectPartnershipApplicationResult {
    pub application: PartnershipApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum RejectPartnershipApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("partnership application is not rejectable")]
    ApplicationNotRejectable,
    #[error("existing listing source not found")]
    ListingSourceNotFound,
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
    #[error("partnership application notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait RejectPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RejectPartnershipApplicationCommand,
    ) -> Result<RejectPartnershipApplicationResult, RejectPartnershipApplicationError>;
}

pub struct RejectPartnershipApplicationHandler<U, A, P, S, R, N> {
    unit_of_work: U,
    applications: A,
    parties: P,
    sources: S,
    admins: R,
    notifications: N,
}

impl<U, A, P, S, R, N> RejectPartnershipApplicationHandler<U, A, P, S, R, N> {
    pub fn new(
        unit_of_work: U,
        applications: A,
        parties: P,
        sources: S,
        admins: R,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            applications,
            parties,
            sources,
            admins,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, P, S, R, N> RejectPartnershipApplicationUseCase
    for RejectPartnershipApplicationHandler<U, A, P, S, R, N>
where
    U: UnitOfWork,
    A: PartnershipApplicationRepositoryFactory<U::Tx>,
    P: PartyRepositoryFactory<U::Tx>,
    S: ListingSourceRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
{
    #[tracing::instrument(name = "reject_partnership_application", skip_all, fields(partnership_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RejectPartnershipApplicationCommand,
    ) -> Result<RejectPartnershipApplicationResult, RejectPartnershipApplicationError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| RejectPartnershipApplicationError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(command.application_id)
            .await?
            .ok_or(RejectPartnershipApplicationError::NotFound)?;

        let (party_name, listing_source_name, image) = match versioned.value.proposal().clone() {
            PartnershipProposal::ExistingListingSource { listing_source_id } => {
                rejection_snapshot_for_existing_source(
                    &mut tx,
                    &self.parties,
                    &self.sources,
                    listing_source_id,
                )
                .await?
            }
            PartnershipProposal::ProposedListingSource {
                party,
                listing_source,
            } => (
                party.name,
                listing_source.name,
                listing_source.presentation.image,
            ),
        };

        if versioned.value.state() != PartnershipApplicationState::Rejected {
            versioned
                .value
                .reject()
                .map_err(|_| RejectPartnershipApplicationError::ApplicationNotRejectable)?;
            versioned = self
                .applications
                .in_transaction(&mut tx)
                .update(&versioned.value, versioned.version)
                .await?;
        }

        self.notifications
            .in_transaction(&mut tx)
            .create_many(&[rejection_notification(
                versioned.value.id(),
                versioned.value.applicant_user_id(),
                party_name,
                listing_source_name,
                image,
            )])
            .await
            .map_err(|source: NotificationCreationError| {
                RejectPartnershipApplicationError::NotificationCreateFailed {
                    source: box_error(source),
                }
            })?;

        tx.commit()
            .await
            .map_err(|_| RejectPartnershipApplicationError::CommitTransactionFailed)?;
        tracing::info!(event = "partnership_application.rejected", partnership_application_id = %versioned.value.id(), actor_type = context.principal.kind(), outcome = "success");
        Ok(RejectPartnershipApplicationResult {
            application: versioned.value,
        })
    }
}

async fn rejection_snapshot_for_existing_source<Tx, P, S>(
    tx: &mut Tx,
    parties: &P,
    sources: &S,
    listing_source_id: ListingSourceId,
) -> Result<
    (
        PartyName,
        listing_source_core::ListingSourceName,
        Option<url::Url>,
    ),
    RejectPartnershipApplicationError,
>
where
    Tx: Transaction,
    P: PartyRepositoryFactory<Tx>,
    S: ListingSourceRepositoryFactory<Tx>,
{
    let source = sources
        .in_transaction(tx)
        .find_by_id(listing_source_id)
        .await?
        .ok_or(RejectPartnershipApplicationError::ListingSourceNotFound)?
        .source;
    let party = parties
        .in_transaction(tx)
        .find_by_id(source.operator_party_id())
        .await?
        .ok_or(RejectPartnershipApplicationError::ListingSourceNotFound)?
        .party;
    Ok((
        party.name().clone(),
        source.name().clone(),
        source.presentation().image.clone(),
    ))
}

fn rejection_notification(
    application_id: PartnershipApplicationId,
    applicant_user_id: user_core::user_id::UserId,
    party_name: PartyName,
    listing_source_name: listing_source_core::ListingSourceName,
    image: Option<url::Url>,
) -> NewNotification {
    NewNotification {
        notification: Notification::new(
            NotificationId::new(),
            applicant_user_id,
            NotificationContent::PartnershipApplication {
                partnership_application_id: application_id,
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name,
                    listing_source_name,
                    image,
                },
                decision: PartnershipApplicationDecision::Rejected,
            },
        ),
        external_delivery: ExternalDeliveryRequest::Requested,
    }
}

impl From<AdminAuthorizationError> for RejectPartnershipApplicationError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
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

impl From<PartnershipApplicationRepositoryError> for RejectPartnershipApplicationError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<party_service::ports::PartyRepositoryError> for RejectPartnershipApplicationError {
    fn from(value: party_service::ports::PartyRepositoryError) -> Self {
        match value {
            party_service::ports::PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            party_service::ports::PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            party_service::ports::PartyRepositoryError::SlugConflict { source }
            | party_service::ports::PartyRepositoryError::Internal { source } => {
                Self::Internal { source }
            }
            party_service::ports::PartyRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected party concurrency"),
            },
        }
    }
}

impl From<listing_source_service::ports::ListingSourceRepositoryError>
    for RejectPartnershipApplicationError
{
    fn from(value: listing_source_service::ports::ListingSourceRepositoryError) -> Self {
        match value {
            listing_source_service::ports::ListingSourceRepositoryError::TemporarilyUnavailable {
                source,
            } => Self::TemporarilyUnavailable { source },
            listing_source_service::ports::ListingSourceRepositoryError::InvalidPersistedState {
                source,
            } => Self::InvalidPersistedState { source },
            listing_source_service::ports::ListingSourceRepositoryError::SlugConflict { source }
            | listing_source_service::ports::ListingSourceRepositoryError::ShopifyDomainConflict {
                source,
            }
            | listing_source_service::ports::ListingSourceRepositoryError::Internal { source } => {
                Self::Internal { source }
            }
            listing_source_service::ports::ListingSourceRepositoryError::ConcurrencyConflict => {
                Self::Internal {
                    source: static_error("unexpected listing source concurrency"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::{ListingIngestionMethod, ListingSourcePresentation};
    use listing_source_service::ports::{
        ListingSourceRepository, ListingSourceRepositoryError, ListingSourceStorageVersion,
        StoredListingSource,
    };
    use notification_service::ports::notification_creator::NotificationCreationOutcome;
    use party_core::{
        party::{Party, PartyContact},
        party_id::PartyId,
    };
    use party_service::ports::{
        PartyRepository, PartyRepositoryError, PartyStorageVersion, StoredParty,
    };
    use std::sync::{Arc, Mutex, MutexGuard};

    use user_service::ports::{UserAdminActorView, UserAdminReadError, UserAdminReader};

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeTransaction {
        state: Arc<Mutex<FakeState>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.state).begins += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone)]
    struct FakeFactories {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeApplicationRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakePartyRepository;
    struct FakeSourceRepository;
    struct FakeAdminReader;
    struct FakeNotificationCreator {
        state: Arc<Mutex<FakeState>>,
    }

    impl PartnershipApplicationRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipApplicationRepository + 'tx {
            FakeApplicationRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl PartyRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartyRepository + 'tx {
            FakePartyRepository
        }
    }

    impl ListingSourceRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ListingSourceRepository + 'tx {
            FakeSourceRepository
        }
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader
        }
    }

    impl NotificationCreatorFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl NotificationCreator + 'tx {
            FakeNotificationCreator {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipApplicationRepository for FakeApplicationRepository {
        async fn find_by_id(
            &mut self,
            _id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            let version =
                PartnershipApplicationStorageVersion::try_from(1_i64).map_err(|error| {
                    PartnershipApplicationRepositoryError::Internal {
                        source: box_error(error),
                    }
                })?;
            Ok(lock(&self.state)
                .application
                .clone()
                .map(|application| Versioned::new(application, version)))
        }

        async fn find_by_id_for_update(
            &mut self,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            self.find_by_id(id).await
        }

        async fn find_by_user_and_id(
            &mut self,
            user_id: user_core::user_id::UserId,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            Ok(self
                .find_by_id(id)
                .await?
                .filter(|application| application.value.applicant_user_id() == user_id))
        }

        async fn insert(
            &mut self,
            _application: &PartnershipApplication,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            Err(PartnershipApplicationRepositoryError::Internal {
                source: static_error("unexpected application insert"),
            })
        }

        async fn update(
            &mut self,
            application: &PartnershipApplication,
            _expected: PartnershipApplicationStorageVersion,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            let version =
                PartnershipApplicationStorageVersion::try_from(2_i64).map_err(|error| {
                    PartnershipApplicationRepositoryError::Internal {
                        source: box_error(error),
                    }
                })?;
            let mut state = lock(&self.state);
            state.application_updates += 1;
            state.application = Some(application.clone());
            Ok(Versioned::new(application.clone(), version))
        }
    }

    #[async_trait::async_trait]
    impl PartyRepository for FakePartyRepository {
        async fn find_by_id(
            &mut self,
            _id: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("unexpected party read"),
            })
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &party_core::party_slug_id::PartySlugId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("unexpected party read"),
            })
        }

        async fn insert(&mut self, _party: &Party) -> Result<StoredParty, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("unexpected party write"),
            })
        }

        async fn update(
            &mut self,
            _party: &Party,
            _expected_version: PartyStorageVersion,
        ) -> Result<StoredParty, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("unexpected party write"),
            })
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceRepository for FakeSourceRepository {
        async fn find_by_id(
            &mut self,
            _id: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected source read"),
            })
        }

        async fn find_by_slug(
            &mut self,
            _slug: &listing_source_core::ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected source read"),
            })
        }

        async fn insert(
            &mut self,
            _source: &listing_source_core::ListingSource,
            _configuration: &listing_source_service::ports::ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected source write"),
            })
        }

        async fn update(
            &mut self,
            _source: &listing_source_core::ListingSource,
            _configuration: &listing_source_service::ports::ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: application::patch_field::PatchField<&str>,
            _expected: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected source write"),
            })
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: user_core::user_id::UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for FakeNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            let mut state = lock(&self.state);
            state.notification_calls += 1;
            if state.notification_fails {
                return Err(NotificationCreationError::CreateFailed {
                    source: static_error("notification creation failed"),
                });
            }
            state.notifications.extend(
                notifications
                    .iter()
                    .map(|notification| notification.notification.content().clone()),
            );
            Ok(notifications
                .iter()
                .map(|notification| NotificationCreationOutcome::Inserted {
                    notification_id: notification.notification.notification_id(),
                })
                .collect())
        }
    }

    struct FakeState {
        application: Option<PartnershipApplication>,
        begins: usize,
        commits: usize,
        application_updates: usize,
        notification_calls: usize,
        notification_fails: bool,
        notifications: Vec<NotificationContent>,
    }

    fn lock(state: &Arc<Mutex<FakeState>>) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn proposed_application() -> PartnershipApplication {
        PartnershipApplication::rehydrate(
            partnership_core::partnership_application::RehydratedPartnershipApplicationState {
                id: PartnershipApplicationId::new(),
                applicant_user_id: user_core::user_id::UserId::new(),
                state: PartnershipApplicationState::InReview,
                approval_result: None,
                proposal: PartnershipProposal::ProposedListingSource {
                    party: partnership_core::partnership_application::ProposedParty {
                        name: PartyName::try_from("Northwind Antiques")
                            .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                        contact: PartyContact {
                            phone: None,
                            email: None,
                        },
                    },
                    listing_source:
                        partnership_core::partnership_application::ProposedListingSource {
                            name: listing_source_core::ListingSourceName::try_from(
                                "Northwind Source",
                            )
                            .unwrap_or_else(|error| {
                                panic!("invalid test listing source name: {error}")
                            }),
                            presentation: ListingSourcePresentation {
                                url: None,
                                image: None,
                            },
                            requested_ingestion_methods: std::collections::HashSet::from([
                                ListingIngestionMethod::PartnerApi,
                            ]),
                        },
                },
            },
        )
        .unwrap_or_else(|error| panic!("valid test application: {error}"))
    }

    fn handler(
        state: Arc<Mutex<FakeState>>,
    ) -> RejectPartnershipApplicationHandler<
        FakeUnitOfWork,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
    > {
        let factories = FakeFactories {
            state: Arc::clone(&state),
        };
        RejectPartnershipApplicationHandler::new(
            FakeUnitOfWork { state },
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories,
        )
    }

    #[tokio::test]
    async fn should_reject_and_create_notification_in_the_same_transaction() {
        let application = proposed_application();
        let application_id = application.id();
        let state = Arc::new(Mutex::new(FakeState {
            application: Some(application),
            begins: 0,
            commits: 0,
            application_updates: 0,
            notification_calls: 0,
            notification_fails: false,
            notifications: Vec::new(),
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                RejectPartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(RejectPartnershipApplicationResult { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_eq!(1, state.application_updates);
        assert_eq!(1, state.notification_calls);
        assert!(matches!(
            state.notifications.as_slice(),
            [NotificationContent::PartnershipApplication {
                decision: PartnershipApplicationDecision::Rejected,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn should_not_commit_when_notification_creation_fails() {
        let application = proposed_application();
        let application_id = application.id();
        let state = Arc::new(Mutex::new(FakeState {
            application: Some(application),
            begins: 0,
            commits: 0,
            application_updates: 0,
            notification_calls: 0,
            notification_fails: true,
            notifications: Vec::new(),
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                RejectPartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(RejectPartnershipApplicationError::NotificationCreateFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.notification_calls);
        assert_eq!(0, state.commits);
    }
}
