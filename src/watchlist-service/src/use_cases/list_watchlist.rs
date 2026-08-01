use crate::ports::{
    WatchlistProductView, WatchlistReadError, WatchlistReader, WatchlistReaderFactory,
};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListWatchlistRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListWatchlistResult {
    pub entries: Vec<WatchlistProductView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListWatchlistError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary watchlist read failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListWatchlistUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError>;
}

pub struct ListWatchlistHandler<U, R> {
    unit_of_work: U,
    watchlist: R,
}

impl<U, R> ListWatchlistHandler<U, R> {
    pub fn new(unit_of_work: U, watchlist: R) -> Self {
        Self {
            unit_of_work,
            watchlist,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListWatchlistUseCase for ListWatchlistHandler<U, R>
where
    U: UnitOfWork,
    R: WatchlistReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "list_watchlist", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError> {
        authorize_read(context, request.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListWatchlistError::BeginTransactionFailed)?;
        let entries = self
            .watchlist
            .in_transaction(&mut tx)
            .find_for_user(request.user_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListWatchlistError::CommitTransactionFailed)?;

        Ok(ListWatchlistResult { entries })
    }
}

fn authorize_read(context: &OperationContext, user_id: UserId) -> Result<(), ListWatchlistError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListWatchlistError>()
}

impl From<OperationAuthorizationError> for ListWatchlistError {
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

impl From<WatchlistReadError> for ListWatchlistError {
    fn from(error: WatchlistReadError) -> Self {
        match error {
            WatchlistReadError::ReadFailed => Self::TemporarilyUnavailable,
            WatchlistReadError::InvalidPersistedState => Self::InvalidPersistedState,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use crate::ports::{
        WatchlistProductView, WatchlistReadError, WatchlistReader, WatchlistReaderFactory,
        WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
    };
    use common::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use common::product_id::ProductId;
    use common::resource_state::domain::ResourceState;
    use common::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
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
                        .cloned()
                        .map(|entry| WatchlistProductView {
                            entry,
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
            state: ResourceState::Active,
        })
    }

    #[tokio::test]
    async fn should_list_watchlist_for_owner() -> Result<(), String> {
        let user_id = UserId::new();
        let entry = entry(user_id, ProductId::new(), true);
        let state = SharedState::with_entry(entry.clone());

        let result = ListWatchlistHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
        )
        .execute(&context_for_user(user_id), ListWatchlistRequest { user_id })
        .await;

        let result = result.map_err(|error| error.to_string())?;
        assert_eq!(1, result.entries.len());
        assert_eq!(entry, result.entries[0].entry);
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_forbid_delegated_user_without_watchlist_capability() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = ListWatchlistHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory { state },
        )
        .execute(
            &delegated_context(user_id, BTreeSet::new()),
            ListWatchlistRequest { user_id },
        )
        .await;

        assert!(matches!(result, Err(ListWatchlistError::Forbidden)));
    }
}
