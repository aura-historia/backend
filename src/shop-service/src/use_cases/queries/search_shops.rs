use crate::ports::{ShopSearchReadError, ShopSearchReader, ShopSearchReaderFactory};
use application::transaction::{Transaction, UnitOfWork};
use common::domain::Domain;
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use shop_core::{partner_status::ShopPartnerStatus, shop_search::ShopSearch, shop_type::ShopType};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsRequest {
    pub search: ShopSearch,
    pub sort: Option<Sort<shop_core::sort_shop_field::SortShopField>>,
    pub cursor: Option<Cursor<ShopId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub partner_status: ShopPartnerStatus,
    pub domains: Vec<Domain>,
    pub image: Option<Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsResult {
    pub items: Vec<ShopSummary>,
    pub cursor: Cursor<ShopId>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchShopsError {
    #[error("temporary shop search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid shop search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal shop search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError>;
}

pub struct SearchShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchShopsUseCase for SearchShopsHandler<U, R>
where
    U: UnitOfWork,
    R: ShopSearchReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_shops",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchShopsError::BeginTransactionFailed)?;

        let result = self.reader.in_transaction(&mut tx).search(&request).await?;

        tx.commit()
            .await
            .map_err(|_| SearchShopsError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<ShopSearchReadError> for SearchShopsError {
    fn from(error: ShopSearchReadError) -> Self {
        match error {
            ShopSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopSearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            ShopSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{TransactionError, UnitOfWork};
    use common::error::boxed::static_error;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum ReadErrorKind {
        TemporarilyUnavailable,
    }

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        search: usize,
    }

    struct State {
        begin_error: bool,
        commit_error: bool,
        search: SearchShopsResult,
        search_error: Option<ReadErrorKind>,
        last_search_request: Option<SearchShopsRequest>,
        counts: Counts,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                begin_error: false,
                commit_error: false,
                search: search_result(),
                search_error: None,
                last_search_request: None,
                counts: Counts::default(),
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakeSearchReaderFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakeSearchReader {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.begin += 1;
                state.begin_error
            });
            if fail {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.commit += 1;
                state.commit_error
            });
            if fail {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ShopSearchReaderFactory<FakeTx> for FakeSearchReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ShopSearchReader + 'tx {
            FakeSearchReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopSearchReader for FakeSearchReader {
        async fn search(
            &mut self,
            request: &SearchShopsRequest,
        ) -> Result<SearchShopsResult, ShopSearchReadError> {
            with_state(&self.state, |state| {
                state.counts.search += 1;
                state.last_search_request = Some(request.clone());
                match state.search_error {
                    Some(kind) => Err(search_read_error(kind)),
                    None => Ok(state.search.clone()),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_search_shops_and_cover_errors() {
        let state = shared_state();
        let expected = search_result();
        with_state(&state, |state| state.search = expected.clone());
        let handler = SearchShopsHandler::new(uow(&state), search_reader(&state));
        let request = search_request();

        let result = handler.execute(&system_context(), request.clone()).await;

        assert!(matches!(result, Ok(ref value) if value.items == expected.items));
        assert_eq!(
            Some(request),
            with_state(&state, |state| state.last_search_request.clone())
        );
        assert_counts(&state, |counts| assert_eq!(1, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler = SearchShopsHandler::new(uow(&state), search_reader(&state));
        let begin = handler.execute(&system_context(), search_request()).await;
        assert!(matches!(
            begin,
            Err(SearchShopsError::BeginTransactionFailed)
        ));

        let state = shared_state();
        with_state(&state, |state| {
            state.search_error = Some(ReadErrorKind::TemporarilyUnavailable)
        });
        let handler = SearchShopsHandler::new(uow(&state), search_reader(&state));
        let read = handler.execute(&system_context(), search_request()).await;
        assert!(matches!(
            read,
            Err(SearchShopsError::TemporarilyUnavailable { .. })
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.commit_error = true);
        let handler = SearchShopsHandler::new(uow(&state), search_reader(&state));
        let commit = handler.execute(&system_context(), search_request()).await;
        assert!(matches!(
            commit,
            Err(SearchShopsError::CommitTransactionFailed)
        ));
    }

    fn search_reader(state: &Arc<Mutex<State>>) -> FakeSearchReaderFactory {
        FakeSearchReaderFactory {
            state: Arc::clone(state),
        }
    }

    fn uow(state: &Arc<Mutex<State>>) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn shared_state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State::default()))
    }

    fn search_read_error(kind: ReadErrorKind) -> ShopSearchReadError {
        match kind {
            ReadErrorKind::TemporarilyUnavailable => ShopSearchReadError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
        }
    }

    fn search_request() -> SearchShopsRequest {
        SearchShopsRequest {
            search: ShopSearch::default(),
            sort: None,
            cursor: None,
        }
    }

    fn search_result() -> SearchShopsResult {
        SearchShopsResult {
            items: vec![ShopSummary {
                shop_id: ShopId::new(),
                shop_slug_id: ShopSlugId::from("antik-markt"),
                name: ShopName::from("Antik Markt"),
                shop_type: ShopType::CommercialDealer,
                partner_status: ShopPartnerStatus::Scraped,
                domains: Vec::new(),
                image: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            }],
            cursor: Cursor {
                size: 10,
                search_after: None,
            },
            total: Some(1),
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn assert_counts(state: &Arc<Mutex<State>>, assert: impl FnOnce(&Counts)) {
        with_state(state, |state| assert(&state.counts));
    }

    fn with_state<R>(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State) -> R) -> R {
        match state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
