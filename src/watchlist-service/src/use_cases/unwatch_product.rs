use crate::ports::{WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct UnwatchProductListingCommand {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnwatchProductListingResult {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, thiserror::Error)]
pub enum UnwatchProductListingError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("watchlist entry not found")]
    NotFound,
    #[error("watchlist entry changed concurrently")]
    ConcurrencyConflict,
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
pub trait UnwatchProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UnwatchProductListingCommand,
    ) -> Result<UnwatchProductListingResult, UnwatchProductListingError>;
}

pub struct UnwatchProductListingHandler<U, R> {
    unit_of_work: U,
    watchlist: R,
}

impl<U, R> UnwatchProductListingHandler<U, R> {
    pub fn new(unit_of_work: U, watchlist: R) -> Self {
        Self {
            unit_of_work,
            watchlist,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> UnwatchProductListingUseCase for UnwatchProductListingHandler<U, R>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "unwatch_product", skip_all, fields(user_id = %command.user_id, product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UnwatchProductListingCommand,
    ) -> Result<UnwatchProductListingResult, UnwatchProductListingError> {
        authorize_write(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UnwatchProductListingError::BeginTransactionFailed)?;
        let loaded = self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_listing_id)
            .await?
            .ok_or(UnwatchProductListingError::NotFound)?;
        match self
            .watchlist
            .in_transaction(&mut tx)
            .delete(command.user_id, command.product_listing_id, loaded.version)
            .await
        {
            Ok(()) => {}
            Err(WatchlistRepositoryError::ConcurrencyConflict) => {
                tracing::warn!(
                    event = "watchlist_product.unwatch_rejected",
                    actor_type = context.principal.kind(),
                    actor_id = %context.principal.label(),
                    user_id = %command.user_id,
                    product_listing_id = %command.product_listing_id,
                    outcome = "concurrency_conflict",
                );
                return Err(UnwatchProductListingError::ConcurrencyConflict);
            }
            Err(error) => return Err(error.into()),
        }
        tx.commit()
            .await
            .map_err(|_| UnwatchProductListingError::CommitTransactionFailed)?;

        tracing::info!(
            event = "watchlist_product.unwatched",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %command.user_id,
            product_listing_id = %command.product_listing_id,
            outcome = "success",
        );

        Ok(UnwatchProductListingResult {
            user_id: command.user_id,
            product_listing_id: command.product_listing_id,
        })
    }
}

fn authorize_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UnwatchProductListingError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UnwatchProductListingError>()
}

impl From<OperationAuthorizationError> for UnwatchProductListingError {
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

impl From<WatchlistRepositoryError> for UnwatchProductListingError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            WatchlistRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            WatchlistRepositoryError::LookupFailed { source }
            | WatchlistRepositoryError::InsertFailed { source }
            | WatchlistRepositoryError::UpdateFailed { source }
            | WatchlistRepositoryError::DeleteFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            error @ WatchlistRepositoryError::AlreadyExists => Self::TemporarilyUnavailable {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use application::error::static_error;

    use crate::ports::{
        VersionedWatchlistProductListing, WatchlistProductListingView, WatchlistReadError,
        WatchlistReader, WatchlistReaderFactory, WatchlistRepository, WatchlistRepositoryError,
        WatchlistRepositoryFactory, WatchlistStorageVersion,
    };
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use watchlist_core::{NewWatchlistProductListing, WatchlistProductListing, WatchlistState};

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

    #[derive(Clone, Default)]
    struct SharedState {
        entries: Arc<Mutex<Vec<VersionedWatchlistProductListing>>>,
        committed: Arc<Mutex<bool>>,
        updated: Arc<Mutex<usize>>,
        unwatched: Arc<Mutex<usize>>,
        force_concurrency_conflict: bool,
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

        fn unwatched(&self) -> usize {
            self.unwatched.lock().map(|value| *value).unwrap_or(0)
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

    impl<Tx> WatchlistReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
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
            if self.state.force_concurrency_conflict {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
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
                .unwatched
                .lock()
                .map(|mut unwatched| *unwatched += 1)
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
    async fn should_unwatch_product_when_entry_exists() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        let result = UnwatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context_for_user(user_id),
            UnwatchProductListingCommand {
                user_id,
                product_listing_id,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(user_id, result.user_id);
        assert_eq!(product_listing_id, result.product_listing_id);
        assert_eq!(1, state.unwatched());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_concurrency_conflict_when_entry_changes_before_delete() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let mut state = SharedState::with_entry(entry(user_id, product_listing_id, true));
        state.force_concurrency_conflict = true;

        let result = UnwatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
        )
        .execute(
            &context_for_user(user_id),
            UnwatchProductListingCommand {
                user_id,
                product_listing_id,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(UnwatchProductListingError::ConcurrencyConflict)
        ));
        assert_eq!(0, state.unwatched());
        assert!(!state.committed());
    }

    #[tokio::test]
    async fn should_return_not_found_when_delete_entry_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = UnwatchProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory { state },
        )
        .execute(
            &context_for_user(user_id),
            UnwatchProductListingCommand {
                user_id,
                product_listing_id: ProductListingId::new(),
            },
        )
        .await;

        assert!(matches!(result, Err(UnwatchProductListingError::NotFound)));
    }
}
