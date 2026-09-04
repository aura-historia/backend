use crate::ports::{
    ListingSourceSearchReadError, ListingSourceSearchReader, ListingSourceSearchReaderFactory,
};
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::sort::Sort;
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
    ListingSourceSearch, ListingSourceSlugId, ReferralConfiguration, SortListingSourceField,
};
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchListingSourcesRequest {
    pub search: ListingSourceSearch,
    pub sort: Option<Sort<SortListingSourceField>>,
    pub cursor: Option<Cursor<ListingSourceId>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceOperatorSummary {
    pub party_id: PartyId,
    pub party_slug_id: PartySlugId,
    pub name: PartyName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceSearchSummary {
    pub listing_source_id: ListingSourceId,
    pub listing_source_slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator: ListingSourceOperatorSummary,
    pub ingestion_methods: std::collections::HashSet<ListingIngestionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchListingSourcesResult {
    pub items: Vec<ListingSourceSearchSummary>,
    pub cursor: Cursor<ListingSourceId>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchListingSourcesError {
    #[error("authenticated actor required to search listing sources")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary listing source search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin listing source search transaction")]
    BeginTransactionFailed,
    #[error("failed to commit listing source search transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchListingSourcesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchListingSourcesRequest,
    ) -> Result<SearchListingSourcesResult, SearchListingSourcesError>;
}

pub struct SearchListingSourcesHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    check_user_admin: A,
}

impl<U, R, A> SearchListingSourcesHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            reader,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> SearchListingSourcesUseCase for SearchListingSourcesHandler<U, R, A>
where
    U: UnitOfWork,
    R: ListingSourceSearchReaderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "search_listing_sources",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchListingSourcesRequest,
    ) -> Result<SearchListingSourcesResult, SearchListingSourcesError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchListingSourcesError::BeginTransactionFailed)?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchListingSourcesError::CommitTransactionFailed)?;

        Ok(result)
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), SearchListingSourcesError>
where
    A: CheckUserAdminUseCase,
{
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map_admin_error),
        Principal::Anonymous => Err(SearchListingSourcesError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> SearchListingSourcesError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            SearchListingSourcesError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => SearchListingSourcesError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            SearchListingSourcesError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => {
            SearchListingSourcesError::Internal { source }
        }
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            SearchListingSourcesError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<ListingSourceSearchReadError> for SearchListingSourcesError {
    fn from(error: ListingSourceSearchReadError) -> Self {
        match error {
            ListingSourceSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ListingSourceSearchReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            ListingSourceSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ListingSourceSearchReader, ListingSourceSearchReaderFactory};
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex};
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        begins: usize,
        searches: usize,
        commits: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.begins += 1;
            drop(state);
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    #[derive(Clone)]
    struct FakeReaderFactory(Arc<Mutex<State>>);

    struct FakeReader(Arc<Mutex<State>>);

    impl ListingSourceSearchReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ListingSourceSearchReader + 'tx {
            FakeReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceSearchReader for FakeReader {
        async fn search(
            &mut self,
            _request: &SearchListingSourcesRequest,
        ) -> Result<SearchListingSourcesResult, ListingSourceSearchReadError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.searches += 1;
            drop(state);
            Ok(SearchListingSourcesResult {
                items: vec![],
                cursor: Cursor::default(),
                total: None,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAdmin {
        allowed: bool,
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for FakeAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.allowed {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request() -> SearchListingSourcesRequest {
        SearchListingSourcesRequest {
            search: ListingSourceSearch::default(),
            sort: None,
            cursor: None,
        }
    }

    #[tokio::test]
    async fn should_authorize_admin_before_searching_and_commit_result() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchListingSourcesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: true },
        );

        let result = handler
            .execute(&context(Principal::User(UserId::new())), request())
            .await;

        assert!(result.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(1, state.searches);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_beginning_listing_source_search_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchListingSourcesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: false },
        );

        let result = handler
            .execute(&context(Principal::User(UserId::new())), request())
            .await;

        assert!(matches!(result, Err(SearchListingSourcesError::Forbidden)));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(0, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_checking_admin_or_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchListingSourcesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: true },
        );

        let result = handler
            .execute(&context(Principal::Anonymous), request())
            .await;

        assert!(matches!(
            result,
            Err(SearchListingSourcesError::AuthenticatedActorRequired)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(0, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }
}
