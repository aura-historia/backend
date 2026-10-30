use crate::ports::{
    ListingSourceSearchReadError, ListingSourceSearchReader, ListingSourceSearchReaderFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
};
use application::pagination::{Cursor, KeysetCursor, ScopeBinding, SortKey};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::sort::{Sort, SortOrder};
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
    ListingSourceSearch, ListingSourceSlugId, PlatformTechnology, ReferralConfiguration,
    SortListingSourceField,
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
    pub cursor: Option<Cursor<ListingSourceSearchCursor>>,
}

impl SearchListingSourcesRequest {
    /// The requested sort, or name ascending.
    pub fn effective_sort(&self) -> Sort<SortListingSourceField> {
        self.sort.unwrap_or(Sort {
            sort: SortListingSourceField::default(),
            order: SortOrder::Asc,
        })
    }

    pub fn scope_binding(&self) -> ScopeBinding {
        let search = &self.search;
        let sort = self.effective_sort();
        ScopeBinding::builder("listing-source-search/v1")
            .optional_part(search.query.as_deref())
            .optional_part(search.name_query.as_deref())
            .optional_part(
                search
                    .listing_source_id
                    .map(|id| id.into_uuid().into_bytes()),
            )
            .optional_part(
                search
                    .listing_source_slug_id
                    .as_ref()
                    .map(AsRef::<str>::as_ref),
            )
            .optional_part(
                search
                    .operator_party_id
                    .map(|id| id.into_uuid().into_bytes()),
            )
            .optional_part(
                search
                    .platform_technology
                    .as_ref()
                    .map(AsRef::<str>::as_ref),
            )
            .optional_part(search.ingestion_method.map(ListingIngestionMethod::as_str))
            .part(sort_field_code(sort.sort))
            .part(sort.order.as_str())
            .finish()
    }

    fn continuation_in_scope(&self) -> bool {
        self.cursor
            .as_ref()
            .and_then(|cursor| cursor.search_after.as_ref())
            .is_none_or(|after| after.continues(&self.scope_binding(), self.effective_sort().sort))
    }
}

fn sort_field_code(field: SortListingSourceField) -> &'static str {
    match field {
        SortListingSourceField::Name => "name",
        SortListingSourceField::Slug => "slug",
        SortListingSourceField::Created => "created",
        SortListingSourceField::Updated => "updated",
    }
}

pub type ListingSourceSearchCursor = KeysetCursor<ListingSourceSortKey, ListingSourceId>;

/// Stored value of the sorted field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingSourceSortKey {
    Name(String),
    Slug(String),
    Created(OffsetDateTime),
    Updated(OffsetDateTime),
}

impl SortKey for ListingSourceSortKey {
    type Field = SortListingSourceField;

    fn field(&self) -> SortListingSourceField {
        match self {
            Self::Name(_) => SortListingSourceField::Name,
            Self::Slug(_) => SortListingSourceField::Slug,
            Self::Created(_) => SortListingSourceField::Created,
            Self::Updated(_) => SortListingSourceField::Updated,
        }
    }
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
    pub platform_technology: Option<PlatformTechnology>,
    pub ingestion_methods: std::collections::HashSet<ListingIngestionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchListingSourcesResult {
    pub items: Vec<ListingSourceSearchSummary>,
    pub cursor: Cursor<ListingSourceSearchCursor>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchListingSourcesError {
    #[error("authenticated actor required to search listing sources")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source search cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
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
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit listing source search transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
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
        if !request.continuation_in_scope() {
            return Err(SearchListingSourcesError::CursorScopeMismatch);
        }

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            SearchListingSourcesError::BeginTransactionFailed(Box::new(source))
        })?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit().await.map_err(|source| {
            SearchListingSourcesError::CommitTransactionFailed(Box::new(source))
        })?;

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
    context
        .principal
        .require_credential_capability(CredentialCapability::ListingSourcesRead)
        .map_err(|error| match error {
            CredentialAuthorizationError::AuthenticationRequired(_) => {
                SearchListingSourcesError::AuthenticatedActorRequired
            }
            CredentialAuthorizationError::InsufficientCapability { .. } => {
                SearchListingSourcesError::Forbidden
            }
        })?;
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
        CheckUserAdminError::BeginTransactionFailed(source)
        | CheckUserAdminError::CommitTransactionFailed(source) => {
            SearchListingSourcesError::TemporarilyUnavailable { source }
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
    struct ScopeAdminCheck(bool);

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for ScopeAdminCheck {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<
            user_service::use_cases::queries::check_user_admin::CheckUserAdminResult,
            CheckUserAdminError,
        > {
            if self.0 {
                Ok(user_service::use_cases::queries::check_user_admin::CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    #[tokio::test]
    async fn should_require_matching_scope_and_admin_role() {
        for (capabilities, admin, allowed) in [
            (std::collections::BTreeSet::new(), true, false),
            (
                std::collections::BTreeSet::from([CredentialCapability::ListingSourcesWrite]),
                true,
                false,
            ),
            (
                std::collections::BTreeSet::from([CredentialCapability::ListingSourcesRead]),
                false,
                false,
            ),
            (
                std::collections::BTreeSet::from([CredentialCapability::ListingSourcesRead]),
                true,
                true,
            ),
        ] {
            let context = OperationContext {
                principal: Principal::DelegatedUser {
                    user_id: user_core::user_id::UserId::new(),
                    capabilities,
                },
                request_id: application::operation_context::RequestId::new("scope-test"),
                correlation_id: application::operation_context::CorrelationId::new("scope-test"),
            };
            let result = ensure_admin_or_internal(&context, &ScopeAdminCheck(admin)).await;
            assert_eq!(allowed, result.is_ok());
            if !allowed {
                assert!(matches!(result, Err(SearchListingSourcesError::Forbidden)));
            }
        }
    }

    fn continuation(
        request: &SearchListingSourcesRequest,
        sort_key: ListingSourceSortKey,
    ) -> SearchListingSourcesRequest {
        SearchListingSourcesRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: Some(ListingSourceSearchCursor {
                    scope: request.scope_binding(),
                    sort_key,
                    id: ListingSourceId::new(),
                }),
            }),
            ..request.clone()
        }
    }

    #[test]
    fn should_bind_cursor_to_effective_filters_and_sort() {
        let name = ListingSourceSortKey::Name("Antik".to_owned());
        let operator = SearchListingSourcesRequest {
            search: ListingSourceSearch {
                operator_party_id: Some(PartyId::new()),
                ..ListingSourceSearch::default()
            },
            ..request()
        };
        let slug_desc = SearchListingSourcesRequest {
            sort: Some(Sort {
                sort: SortListingSourceField::Slug,
                order: SortOrder::Desc,
            }),
            ..request()
        };

        assert_eq!(
            request().scope_binding(),
            SearchListingSourcesRequest {
                sort: Some(Sort {
                    sort: SortListingSourceField::Name,
                    order: SortOrder::Asc,
                }),
                ..request()
            }
            .scope_binding()
        );
        assert!(continuation(&operator, name.clone()).continuation_in_scope());
        assert!(
            !SearchListingSourcesRequest {
                search: ListingSourceSearch {
                    operator_party_id: Some(PartyId::new()),
                    ..ListingSourceSearch::default()
                },
                ..continuation(&operator, name.clone())
            }
            .continuation_in_scope()
        );
        assert!(
            !SearchListingSourcesRequest {
                sort: slug_desc.sort,
                ..continuation(&request(), name)
            }
            .continuation_in_scope()
        );
        assert!(
            !continuation(&slug_desc, ListingSourceSortKey::Name("Antik".to_owned()))
                .continuation_in_scope()
        );
    }

    #[tokio::test]
    async fn should_reject_cross_scope_cursor_before_beginning_listing_source_search_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = SearchListingSourcesHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory(Arc::clone(&state)),
            FakeAdmin { allowed: true },
        );
        let foreign = SearchListingSourcesRequest {
            sort: Some(Sort {
                sort: SortListingSourceField::Created,
                order: SortOrder::Desc,
            }),
            ..continuation(&request(), ListingSourceSortKey::Name("Antik".to_owned()))
        };

        let result = handler
            .execute(&context(Principal::User(UserId::new())), foreign)
            .await;

        assert!(matches!(
            result,
            Err(SearchListingSourcesError::CursorScopeMismatch)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(0, state.begins);
        assert_eq!(0, state.searches);
    }
}
