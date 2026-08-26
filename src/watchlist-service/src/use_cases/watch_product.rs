use crate::ports::{
    WatchlistQuotaReadError, WatchlistQuotaReader, WatchlistQuotaReaderFactory,
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
};
use crate::tier_policy::active_watchlist_quota;
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};
use watchlist_core::watchlist_state::WatchlistState;
use watchlist_core::{NewWatchlistProductListing, WatchlistProductListing};

#[derive(Debug, Clone, PartialEq)]
pub struct WatchProductListingCommand {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
    pub notifications: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchProductListingResult {
    pub entry: WatchlistProductListing,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchProductListingError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("watchlist entry already exists")]
    AlreadyExists,
    #[error("user not found")]
    UserNotFound,
    #[error("watchlist quota exceeded: {active_count}/{quota} active entries are already in use")]
    WatchlistQuotaExceeded { active_count: usize, quota: usize },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist quota read failed")]
    WatchlistQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("temporary watchlist persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait WatchProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WatchProductListingCommand,
    ) -> Result<WatchProductListingResult, WatchProductListingError>;
}

pub struct WatchProductListingHandler<U, R, Q, A> {
    unit_of_work: U,
    watchlist: R,
    quotas: Q,
    tier_entitlements: A,
}

impl<U, R, Q, A> WatchProductListingHandler<U, R, Q, A> {
    pub fn new(unit_of_work: U, watchlist: R, quotas: Q, tier_entitlements: A) -> Self {
        Self {
            unit_of_work,
            watchlist,
            quotas,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, Q, A> WatchProductListingUseCase for WatchProductListingHandler<U, R, Q, A>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
    Q: WatchlistQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(name = "watch_product", skip_all, fields(user_id = %command.user_id, product_listing_id = %command.product_listing_id, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: WatchProductListingCommand,
    ) -> Result<WatchProductListingResult, WatchProductListingError> {
        authorize_watch(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WatchProductListingError::BeginTransactionFailed)?;
        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(command.user_id)
            .await
            .map_err(tier_entitlements_error)?
            .ok_or(WatchProductListingError::UserNotFound)?;
        if self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_listing_id)
            .await?
            .is_some()
        {
            return Err(WatchProductListingError::AlreadyExists);
        }
        if let Some(quota) = active_watchlist_quota(tier) {
            let active_count = self
                .quotas
                .in_transaction(&mut tx)
                .count_active_for_user(command.user_id)
                .await
                .map_err(watchlist_quota_read_error)?;
            if active_count >= quota {
                return Err(WatchProductListingError::WatchlistQuotaExceeded {
                    active_count,
                    quota,
                });
            }
        }

        let entry = WatchlistProductListing::create(NewWatchlistProductListing {
            user_id: command.user_id,
            product_listing_id: command.product_listing_id,
            notifications: command.notifications,
            state: WatchlistState::Active,
        });
        let entry = self
            .watchlist
            .in_transaction(&mut tx)
            .insert(&entry)
            .await?
            .into_value();
        tx.commit()
            .await
            .map_err(|_| WatchProductListingError::CommitTransactionFailed)?;
        tracing::info!(
            event = "watchlist_product.watched",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            user_id = %command.user_id,
            product_listing_id = %command.product_listing_id,
            outcome = "success",
        );
        Ok(WatchProductListingResult { entry })
    }
}

fn authorize_watch(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), WatchProductListingError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<WatchProductListingError>()
}

impl From<OperationAuthorizationError> for WatchProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> WatchProductListingError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            WatchProductListingError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn watchlist_quota_read_error(error: WatchlistQuotaReadError) -> WatchProductListingError {
    WatchProductListingError::WatchlistQuotaReadFailed {
        source: box_error(error),
    }
}

impl From<WatchlistRepositoryError> for WatchProductListingError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::AlreadyExists => Self::AlreadyExists,
            WatchlistRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            error @ WatchlistRepositoryError::ConcurrencyConflict => Self::TemporarilyUnavailable {
                source: box_error(error),
            },
            WatchlistRepositoryError::LookupFailed { source }
            | WatchlistRepositoryError::InsertFailed { source }
            | WatchlistRepositoryError::UpdateFailed { source }
            | WatchlistRepositoryError::DeleteFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use application::error::static_error;

    use crate::ports::{
        VersionedWatchlistProductListing, WatchlistProductListingView, WatchlistQuotaReadError,
        WatchlistQuotaReader, WatchlistQuotaReaderFactory, WatchlistReadError, WatchlistReader,
        WatchlistReaderFactory, WatchlistRepository, WatchlistRepositoryError,
        WatchlistRepositoryFactory, WatchlistStorageVersion,
    };
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_core::tier::UserTier;
    use user_service::ports::{
        UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
    };
    use watchlist_core::watchlist_state::WatchlistState;
    use watchlist_core::{NewWatchlistProductListing, WatchlistProductListing};

    #[derive(Clone, Default)]
    struct TestUnitOfWork {
        state: SharedState,
        fail_begin: bool,
        fail_commit: bool,
    }

    struct TestTransaction {
        state: SharedState,
        fail_commit: bool,
    }

    #[derive(Clone, Default)]
    struct TestWatchlistFactory {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct TestAccountFactory {
        tier: UserTier,
    }

    struct TestAccountReader {
        tier: UserTier,
    }

    #[derive(Clone, Default)]
    struct SharedState {
        entries: Arc<Mutex<Vec<VersionedWatchlistProductListing>>>,
        committed: Arc<Mutex<bool>>,
        updated: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<usize>>,
    }

    struct TestWatchlistPort {
        state: SharedState,
    }

    impl SharedState {
        fn with_entry(entry: WatchlistProductListing) -> Self {
            let state = Self::default();
            state.push(entry);
            state
        }

        fn push(&self, entry: WatchlistProductListing) {
            if let Ok(mut entries) = self.entries.lock() {
                entries.push(VersionedWatchlistProductListing::new(
                    entry,
                    WatchlistStorageVersion::INITIAL,
                ));
            }
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
            if self.fail_commit {
                return Err(TransactionError::CommitFailed);
            }
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
            if self.fail_begin {
                return Err(TransactionError::BeginFailed);
            }
            Ok(TestTransaction {
                state: self.state.clone(),
                fail_commit: self.fail_commit,
            })
        }
    }

    impl<Tx> WatchlistRepositoryFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistRepository + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    impl<Tx> WatchlistQuotaReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistQuotaReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    impl<Tx> UserTierEntitlementsFactory<Tx> for TestAccountFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl UserTierEntitlements + 'tx {
            TestAccountReader { tier: self.tier }
        }
    }

    impl<Tx> WatchlistReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserTierEntitlements for TestAccountReader {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            Ok(Some(self.tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl WatchlistQuotaReader for TestWatchlistPort {
        async fn count_active_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<usize, WatchlistQuotaReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_poisoned| WatchlistQuotaReadError::ReadFailed {
                    source: static_error("watchlist quota test state mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| {
                            entry.value.user_id() == user_id && entry.value.state().is_active()
                        })
                        .count()
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistRepository for TestWatchlistPort {
        async fn find_by_user_and_product(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
        ) -> Result<Option<VersionedWatchlistProductListing>, WatchlistRepositoryError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::LookupFailed {
                    source: static_error("watchlist test lookup mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .find(|entry| {
                            entry.value.user_id() == user_id
                                && entry.value.product_listing_id() == product_listing_id
                        })
                        .cloned()
                })
        }

        async fn insert(
            &mut self,
            entry: &WatchlistProductListing,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::InsertFailed {
                        source: static_error("watchlist test insert mutex is poisoned"),
                    })?;
            if entries.iter().any(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) {
                return Err(WatchlistRepositoryError::AlreadyExists);
            }
            let persisted = VersionedWatchlistProductListing::new(
                entry.clone(),
                WatchlistStorageVersion::INITIAL,
            );
            entries.push(persisted.clone());
            Ok(persisted)
        }

        async fn update(
            &mut self,
            entry: &WatchlistProductListing,
            expected_version: WatchlistStorageVersion,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                        source: static_error("watchlist test update mutex is poisoned"),
                    })?;
            let Some(existing) = entries.iter_mut().find(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) else {
                return Err(WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test entry is missing"),
                });
            };
            if existing.version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            let persisted =
                VersionedWatchlistProductListing::new(entry.clone(), expected_version.next());
            *existing = persisted.clone();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test update counter mutex is poisoned"),
                })?;
            Ok(persisted)
        }

        async fn delete(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
            expected_version: WatchlistStorageVersion,
        ) -> Result<(), WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                        source: static_error("watchlist test delete mutex is poisoned"),
                    })?;
            let Some(index) = entries.iter().position(|entry| {
                entry.value.user_id() == user_id
                    && entry.value.product_listing_id() == product_listing_id
            }) else {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            };
            if entries[index].version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            entries.remove(index);
            self.state
                .deleted
                .lock()
                .map(|mut deleted| *deleted += 1)
                .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                    source: static_error("watchlist test delete counter mutex is poisoned"),
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistReader for TestWatchlistPort {
        async fn find_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<WatchlistProductListingView>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.user_id() == user_id)
                        .map(|entry| WatchlistProductListingView {
                            user_id: entry.value.user_id(),
                            product_listing_id: entry.value.product_listing_id(),
                            notifications: entry.value.notifications(),
                            state: entry.value.state(),
                            created: OffsetDateTime::UNIX_EPOCH,
                            updated: OffsetDateTime::UNIX_EPOCH,
                        })
                        .collect()
                })
        }

        async fn find_user_ids_for_product(
            &mut self,
            product_listing_id: ProductListingId,
        ) -> Result<Vec<UserId>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.product_listing_id() == product_listing_id)
                        .map(|entry| entry.value.user_id())
                        .collect()
                })
        }
    }

    fn context_for_user(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn delegated_context(
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    ) -> OperationContext {
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities,
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn entry(
        user_id: UserId,
        product_listing_id: ProductListingId,
        notifications: bool,
    ) -> WatchlistProductListing {
        WatchlistProductListing::create(NewWatchlistProductListing {
            user_id,
            product_listing_id,
            notifications,
            state: WatchlistState::Active,
        })
    }

    #[tokio::test]
    async fn should_watch_product_when_entry_missing() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::default();

        let result = WatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestAccountFactory {
                tier: UserTier::Free,
            },
        )
        .execute(
            &context_for_user(user_id),
            WatchProductListingCommand {
                user_id,
                product_listing_id,
                notifications: true,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(user_id, result.entry.user_id());
        assert_eq!(product_listing_id, result.entry.product_listing_id());
        assert!(result.entry.notifications());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_already_exists_when_entry_exists() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        let result = WatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory { state },
            TestAccountFactory {
                tier: UserTier::Free,
            },
        )
        .execute(
            &context_for_user(user_id),
            WatchProductListingCommand {
                user_id,
                product_listing_id,
                notifications: true,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(WatchProductListingError::AlreadyExists)
        ));
    }

    #[tokio::test]
    async fn should_reject_pro_tier_watch_when_active_quota_is_reached() {
        let user_id = UserId::new();
        let state = SharedState::default();
        for _ in 0..100 {
            state.push(entry(user_id, ProductListingId::new(), true));
        }

        let result = WatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestAccountFactory {
                tier: UserTier::Pro,
            },
        )
        .execute(
            &context_for_user(user_id),
            WatchProductListingCommand {
                user_id,
                product_listing_id: ProductListingId::new(),
                notifications: true,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(WatchProductListingError::WatchlistQuotaExceeded {
                active_count: 100,
                quota: 100,
            })
        ));
        assert!(!state.committed());
    }

    #[tokio::test]
    async fn should_forbid_delegated_user_without_watchlist_write() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = WatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory { state },
            TestAccountFactory {
                tier: UserTier::Free,
            },
        )
        .execute(
            &delegated_context(user_id, BTreeSet::new()),
            WatchProductListingCommand {
                user_id,
                product_listing_id: ProductListingId::new(),
                notifications: true,
            },
        )
        .await;

        assert!(matches!(result, Err(WatchProductListingError::Forbidden)));
    }
}
