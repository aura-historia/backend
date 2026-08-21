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
use product_core::product_id::ProductId;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};
use watchlist_core::WatchlistProduct;
use watchlist_core::watchlist_state::WatchlistState;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductCommand {
    pub user_id: UserId,
    pub product_id: ProductId,
    pub notifications: Option<bool>,
    pub state: Option<WatchlistState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductResult {
    pub entry: WatchlistProduct,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateWatchlistProductError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("watchlist entry not found")]
    NotFound,
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
    TemporarilyUnavailable,
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateWatchlistProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductCommand,
    ) -> Result<UpdateWatchlistProductResult, UpdateWatchlistProductError>;
}

pub struct UpdateWatchlistProductHandler<U, R, Q, A> {
    unit_of_work: U,
    watchlist: R,
    quotas: Q,
    tier_entitlements: A,
}

impl<U, R, Q, A> UpdateWatchlistProductHandler<U, R, Q, A> {
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
impl<U, R, Q, A> UpdateWatchlistProductUseCase for UpdateWatchlistProductHandler<U, R, Q, A>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
    Q: WatchlistQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(name = "update_watchlist_product", skip_all, fields(user_id = %command.user_id, product_id = %command.product_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductCommand,
    ) -> Result<UpdateWatchlistProductResult, UpdateWatchlistProductError> {
        authorize_write(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateWatchlistProductError::BeginTransactionFailed)?;
        let mut entry = self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_id)
            .await?
            .ok_or(UpdateWatchlistProductError::NotFound)?;

        let reactivating =
            matches!(command.state, Some(WatchlistState::Active)) && !entry.state().is_active();
        if reactivating {
            let tier = self
                .tier_entitlements
                .in_transaction(&mut tx)
                .lock_user_tier(command.user_id)
                .await
                .map_err(tier_entitlements_error)?
                .ok_or(UpdateWatchlistProductError::UserNotFound)?;
            if let Some(quota) = active_watchlist_quota(tier) {
                let active_count = self
                    .quotas
                    .in_transaction(&mut tx)
                    .count_active_for_user(command.user_id)
                    .await
                    .map_err(watchlist_quota_read_error)?;
                if active_count >= quota {
                    return Err(UpdateWatchlistProductError::WatchlistQuotaExceeded {
                        active_count,
                        quota,
                    });
                }
            }
        }
        let mut changed = false;
        if let Some(notifications) = command.notifications
            && entry.notifications() != notifications
        {
            entry.change_notifications(notifications);
            changed = true;
        }
        if let Some(state) = command.state
            && entry.state() != state
        {
            entry.change_state(state);
            changed = true;
        }
        if changed {
            entry = self
                .watchlist
                .in_transaction(&mut tx)
                .update(&entry)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| UpdateWatchlistProductError::CommitTransactionFailed)?;
        tracing::info!(
            event = "watchlist_product.updated",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            user_id = %command.user_id,
            product_id = %command.product_id,
            outcome = if changed { "success" } else { "unchanged" },
        );
        Ok(UpdateWatchlistProductResult { entry })
    }
}

fn authorize_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateWatchlistProductError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateWatchlistProductError>()
}

impl From<OperationAuthorizationError> for UpdateWatchlistProductError {
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

fn tier_entitlements_error(error: UserTierEntitlementsError) -> UpdateWatchlistProductError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            UpdateWatchlistProductError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn watchlist_quota_read_error(error: WatchlistQuotaReadError) -> UpdateWatchlistProductError {
    UpdateWatchlistProductError::WatchlistQuotaReadFailed {
        source: box_error(error),
    }
}

impl From<WatchlistRepositoryError> for UpdateWatchlistProductError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            WatchlistRepositoryError::LookupFailed
            | WatchlistRepositoryError::InsertFailed
            | WatchlistRepositoryError::UpdateFailed
            | WatchlistRepositoryError::DeleteFailed => Self::TemporarilyUnavailable,
            WatchlistRepositoryError::AlreadyExists => Self::TemporarilyUnavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use application::error::static_error;

    use crate::ports::{
        WatchlistProductView, WatchlistQuotaReadError, WatchlistQuotaReader,
        WatchlistQuotaReaderFactory, WatchlistReadError, WatchlistReader, WatchlistReaderFactory,
        WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
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
    use watchlist_core::{NewWatchlistProduct, WatchlistProduct};

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
        entries: Arc<Mutex<Vec<WatchlistProduct>>>,
        committed: Arc<Mutex<bool>>,
        updated: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<usize>>,
    }

    struct TestWatchlistPort {
        state: SharedState,
    }

    impl SharedState {
        fn with_entry(entry: WatchlistProduct) -> Self {
            let state = Self::default();
            state.push(entry);
            state
        }

        fn push(&self, entry: WatchlistProduct) {
            if let Ok(mut entries) = self.entries.lock() {
                entries.push(entry);
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
                        .filter(|entry| entry.user_id() == user_id && entry.state().is_active())
                        .count()
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistRepository for TestWatchlistPort {
        async fn find_by_user_and_product(
            &mut self,
            user_id: UserId,
            product_id: ProductId,
        ) -> Result<Option<WatchlistProduct>, WatchlistRepositoryError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::LookupFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .find(|entry| {
                            entry.user_id() == user_id && entry.product_id() == product_id
                        })
                        .cloned()
                })
        }

        async fn insert(
            &mut self,
            entry: &WatchlistProduct,
        ) -> Result<WatchlistProduct, WatchlistRepositoryError> {
            let mut entries = self
                .state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::InsertFailed)?;
            if entries.iter().any(|existing| {
                existing.user_id() == entry.user_id() && existing.product_id() == entry.product_id()
            }) {
                return Err(WatchlistRepositoryError::AlreadyExists);
            }
            entries.push(entry.clone());
            Ok(entry.clone())
        }

        async fn update(
            &mut self,
            entry: &WatchlistProduct,
        ) -> Result<WatchlistProduct, WatchlistRepositoryError> {
            let mut entries = self
                .state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::UpdateFailed)?;
            let Some(existing) = entries.iter_mut().find(|existing| {
                existing.user_id() == entry.user_id() && existing.product_id() == entry.product_id()
            }) else {
                return Err(WatchlistRepositoryError::UpdateFailed);
            };
            *existing = entry.clone();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| WatchlistRepositoryError::UpdateFailed)?;
            Ok(entry.clone())
        }

        async fn delete(
            &mut self,
            user_id: UserId,
            product_id: ProductId,
        ) -> Result<(), WatchlistRepositoryError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::DeleteFailed)?
                .retain(|entry| entry.user_id() != user_id || entry.product_id() != product_id);
            self.state
                .deleted
                .lock()
                .map(|mut deleted| *deleted += 1)
                .map_err(|_| WatchlistRepositoryError::DeleteFailed)
        }
    }

    #[async_trait::async_trait]
    impl WatchlistReader for TestWatchlistPort {
        async fn find_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<WatchlistProductView>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.user_id() == user_id)
                        .map(|entry| WatchlistProductView {
                            user_id: entry.user_id(),
                            product_id: entry.product_id(),
                            notifications: entry.notifications(),
                            state: entry.state(),
                            created: OffsetDateTime::UNIX_EPOCH,
                            updated: OffsetDateTime::UNIX_EPOCH,
                        })
                        .collect()
                })
        }

        async fn find_user_ids_for_product(
            &mut self,
            product_id: ProductId,
        ) -> Result<Vec<UserId>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.product_id() == product_id)
                        .map(WatchlistProduct::user_id)
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

    fn entry(user_id: UserId, product_id: ProductId, notifications: bool) -> WatchlistProduct {
        WatchlistProduct::create(NewWatchlistProduct {
            user_id,
            product_id,
            notifications,
            state: WatchlistState::Active,
        })
    }

    #[tokio::test]
    async fn should_update_notifications_when_entry_exists() -> Result<(), String> {
        let user_id = UserId::new();
        let product_id = ProductId::new();
        let state = SharedState::with_entry(entry(user_id, product_id, true));

        let result = UpdateWatchlistProductHandler::new(
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
            UpdateWatchlistProductCommand {
                user_id,
                product_id,
                notifications: Some(false),
                state: None,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert!(!result.entry.notifications());
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_update_entry_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = UpdateWatchlistProductHandler::new(
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
            UpdateWatchlistProductCommand {
                user_id,
                product_id: ProductId::new(),
                notifications: Some(false),
                state: None,
            },
        )
        .await;

        assert!(matches!(result, Err(UpdateWatchlistProductError::NotFound)));
    }

    #[tokio::test]
    async fn should_forbid_delegated_user_without_watchlist_write() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = UpdateWatchlistProductHandler::new(
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
            UpdateWatchlistProductCommand {
                user_id,
                product_id: ProductId::new(),
                notifications: Some(false),
                state: None,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductError::Forbidden)
        ));
    }
}
