use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{
        ContentNoticeAdminSearchReadError, ContentNoticeAdminSearchReader,
        ContentNoticeAdminSearchReaderFactory,
    },
};
use application::{
    error::BoxError,
    operation_context::{CredentialCapability, OperationContext},
    pagination::{Cursor, CursoredResult, KeysetCursor, ScopeBinding, SortKey},
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{
        ContentNoticeTarget, ContentNoticeTargetType, ProductListingTitleSnapshot,
    },
    reason_category::ReasonCategory,
    submitter::{SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use domain_primitives::{
    query::range_query::RangeQuery,
    sort::{Sort, SortOrder},
};
use listing_source_core::{ListingSourceId, ListingSourceName};
use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;
use user_service::ports::UserAdminReaderFactory;

/// Filters of the admin review queue. Personal data is deliberately not filterable, so it never
/// appears in URLs or access logs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContentNoticeSearch {
    pub target_type: Option<ContentNoticeTargetType>,
    /// Matches notices about the listing source itself and about any of its listings.
    pub listing_source_id: Option<ListingSourceId>,
    pub product_listing_id: Option<ProductListingId>,
    pub reason_category: Option<ReasonCategory>,
    pub submitter_role: Option<SubmitterRole>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortContentNoticeField {
    #[default]
    Created,
}

impl SortContentNoticeField {
    fn code(self) -> &'static str {
        match self {
            Self::Created => "created",
        }
    }
}

/// Stored value of the sorted field at a cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentNoticeSortKey {
    Created(OffsetDateTime),
}

impl SortKey for ContentNoticeSortKey {
    type Field = SortContentNoticeField;

    fn field(&self) -> SortContentNoticeField {
        match self {
            Self::Created(_) => SortContentNoticeField::Created,
        }
    }
}

pub type ContentNoticeCursor = KeysetCursor<ContentNoticeSortKey, ContentNoticeId>;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListAdminContentNoticesRequest {
    pub search: ContentNoticeSearch,
    pub sort: Option<Sort<SortContentNoticeField>>,
    pub cursor: Option<Cursor<ContentNoticeCursor>>,
}

impl ListAdminContentNoticesRequest {
    /// The requested sort, or newest first.
    pub fn effective_sort(&self) -> Sort<SortContentNoticeField> {
        self.sort.unwrap_or(Sort {
            sort: SortContentNoticeField::default(),
            order: SortOrder::Desc,
        })
    }

    pub fn scope_binding(&self) -> ScopeBinding {
        let search = &self.search;
        let sort = self.effective_sort();
        ScopeBinding::builder("content-notice-admin-search/v1")
            .optional_part(search.target_type.map(ContentNoticeTargetType::as_str))
            .optional_part(
                search
                    .listing_source_id
                    .map(|id| id.into_uuid().into_bytes()),
            )
            .optional_part(
                search
                    .product_listing_id
                    .map(|id| id.into_uuid().into_bytes()),
            )
            .optional_part(search.reason_category.map(ReasonCategory::as_str))
            .optional_part(search.submitter_role.map(SubmitterRole::as_str))
            .timestamp_range(search.created)
            .part(sort.sort.code())
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

/// One row of the admin review queue. The listing source name, listing title and partner flag
/// are the snapshot taken at submission.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentNoticeSummary {
    pub id: ContentNoticeId,
    pub state: ContentNoticeState,
    pub created: OffsetDateTime,
    pub target: ContentNoticeTarget,
    pub listing_source_name: ListingSourceName,
    /// Only a product listing target can have a title.
    pub product_listing_title: Option<ProductListingTitleSnapshot>,
    pub listing_source_partner: bool,
    pub submitter_role: SubmitterRole,
    pub reason_category: ReasonCategory,
    pub submitter_name: SubmitterName,
    pub submitter_email: SubmitterEmail,
}

pub type ListAdminContentNoticesResult = CursoredResult<ContentNoticeSummary, ContentNoticeCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListAdminContentNoticesError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("content notice search cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
    #[error("temporary content notice search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid content notice search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal content notice search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list admin content notices transaction")]
    BeginTransactionFailed(#[source] BoxError),
    #[error("failed to commit list admin content notices transaction")]
    CommitTransactionFailed(#[source] BoxError),
}

#[async_trait::async_trait]
pub trait ListAdminContentNoticesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminContentNoticesRequest,
    ) -> Result<ListAdminContentNoticesResult, ListAdminContentNoticesError>;
}

pub struct ListAdminContentNoticesHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admins: A,
}

impl<U, R, A> ListAdminContentNoticesHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admins: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> ListAdminContentNoticesUseCase for ListAdminContentNoticesHandler<U, R, A>
where
    U: UnitOfWork,
    R: ContentNoticeAdminSearchReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_admin_content_notices",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            result_count = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminContentNoticesRequest,
    ) -> Result<ListAdminContentNoticesResult, ListAdminContentNoticesError> {
        context
            .principal
            .require_credential_capability(CredentialCapability::ContentNoticesRead)
            .map_err(|_| ListAdminContentNoticesError::Forbidden)?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            ListAdminContentNoticesError::BeginTransactionFailed(Box::new(source))
        })?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        if !request.continuation_in_scope() {
            return Err(ListAdminContentNoticesError::CursorScopeMismatch);
        }
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tracing::Span::current().record("result_count", result.items.len());
        tx.commit().await.map_err(|source| {
            ListAdminContentNoticesError::CommitTransactionFailed(Box::new(source))
        })?;
        Ok(result)
    }
}

impl From<AdminAuthorizationError> for ListAdminContentNoticesError {
    fn from(error: AdminAuthorizationError) -> Self {
        match error {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<ContentNoticeAdminSearchReadError> for ListAdminContentNoticesError {
    fn from(error: ContentNoticeAdminSearchReadError) -> Self {
        match error {
            ContentNoticeAdminSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ContentNoticeAdminSearchReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            ContentNoticeAdminSearchReadError::Internal { source } => Self::Internal { source },
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
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex, MutexGuard},
    };
    use time::macros::datetime;
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{UserAdminActorView, UserAdminReadError, UserAdminReader};

    #[derive(Default)]
    struct State {
        begins: usize,
        searches: usize,
        commits: usize,
        request: Option<ListAdminContentNoticesRequest>,
        fail_begin: bool,
        fail_commit: bool,
    }

    type SharedState = Arc<Mutex<State>>;

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(SharedState);

    struct FakeTransaction(SharedState);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            if state.fail_commit {
                return Err(TransactionError::CommitFailed(static_error(
                    "commit failed",
                )));
            }
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            if state.fail_begin {
                return Err(TransactionError::BeginFailed(static_error("begin failed")));
            }
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    struct FakeReaderFactory(SharedState);

    struct FakeReader(SharedState);

    impl ContentNoticeAdminSearchReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ContentNoticeAdminSearchReader + 'tx {
            FakeReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ContentNoticeAdminSearchReader for FakeReader {
        async fn search(
            &mut self,
            request: &ListAdminContentNoticesRequest,
        ) -> Result<ListAdminContentNoticesResult, ContentNoticeAdminSearchReadError> {
            let mut state = lock(&self.0);
            state.searches += 1;
            state.request = Some(request.clone());
            Ok(CursoredResult::default())
        }
    }

    struct FakeAdminFactory(Option<UserRole>);

    struct FakeAdminReader(Option<UserRole>);

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader(self.0)
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(self.0.map(|role| UserAdminActorView { user_id, role }))
        }
    }

    fn handler(
        state: &SharedState,
        role: Option<UserRole>,
    ) -> ListAdminContentNoticesHandler<FakeUnitOfWork, FakeReaderFactory, FakeAdminFactory> {
        ListAdminContentNoticesHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeReaderFactory(Arc::clone(state)),
            FakeAdminFactory(role),
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn delegated(capabilities: impl IntoIterator<Item = CredentialCapability>) -> Principal {
        Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: capabilities.into_iter().collect::<BTreeSet<_>>(),
        }
    }

    fn filtered_request() -> ListAdminContentNoticesRequest {
        ListAdminContentNoticesRequest {
            search: ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ProductListing),
                listing_source_id: Some(ListingSourceId::new()),
                product_listing_id: Some(ProductListingId::new()),
                reason_category: Some(ReasonCategory::RightsInfringement),
                submitter_role: Some(SubmitterRole::RightsHolder),
                created: Some(RangeQuery {
                    min: Some(datetime!(2026-11-01 00:00 UTC)),
                    max: None,
                }),
            },
            sort: Some(Sort {
                sort: SortContentNoticeField::Created,
                order: SortOrder::Asc,
            }),
            cursor: None,
        }
    }

    fn continued(
        mut request: ListAdminContentNoticesRequest,
        scope: ScopeBinding,
    ) -> ListAdminContentNoticesRequest {
        request.cursor = Some(Cursor {
            size: 2,
            search_after: Some(ContentNoticeCursor {
                scope,
                sort_key: ContentNoticeSortKey::Created(datetime!(2026-11-13 10:44 UTC)),
                id: ContentNoticeId::new(),
            }),
        });
        request
    }

    #[tokio::test]
    async fn should_authorize_admin_search_and_commit_in_one_transaction() {
        let state = SharedState::default();
        let filtered = filtered_request();
        let request = continued(filtered.clone(), filtered.scope_binding());

        let result = handler(&state, Some(UserRole::Admin))
            .execute(
                &context(delegated([CredentialCapability::ContentNoticesRead])),
                request.clone(),
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!((1, 1, 1), (state.begins, state.searches, state.commits));
        assert_eq!(Some(request), state.request);
    }

    #[tokio::test]
    async fn should_forbid_without_capability_admin_role_or_authentication() {
        let cases = [
            (Principal::Anonymous, Some(UserRole::Admin), 0),
            (delegated([]), Some(UserRole::Admin), 0),
            (
                delegated([CredentialCapability::AdminOverviewRead]),
                Some(UserRole::Admin),
                0,
            ),
            (
                delegated([CredentialCapability::ContentNoticesRead]),
                Some(UserRole::User),
                1,
            ),
            (Principal::User(UserId::new()), Some(UserRole::User), 1),
            (Principal::User(UserId::new()), None, 1),
        ];

        for (principal, role, expected_begins) in cases {
            let state = SharedState::default();

            let result = handler(&state, role)
                .execute(&context(principal), filtered_request())
                .await;

            assert!(matches!(
                result,
                Err(ListAdminContentNoticesError::Forbidden)
            ));
            let state = lock(&state);
            assert_eq!(expected_begins, state.begins);
            assert_eq!((0, 0), (state.searches, state.commits));
        }
    }

    #[tokio::test]
    async fn should_reject_cursor_from_another_scope_before_searching() {
        let filtered = filtered_request();
        let other_filters = ListAdminContentNoticesRequest {
            search: ContentNoticeSearch {
                reason_category: Some(ReasonCategory::Other),
                ..filtered.search.clone()
            },
            ..filtered.clone()
        };
        let other_order = ListAdminContentNoticesRequest {
            sort: None,
            ..filtered.clone()
        };

        for foreign_scope in [other_filters.scope_binding(), other_order.scope_binding()] {
            let state = SharedState::default();

            let result = handler(&state, Some(UserRole::Admin))
                .execute(
                    &context(Principal::User(UserId::new())),
                    continued(filtered.clone(), foreign_scope),
                )
                .await;

            assert!(matches!(
                result,
                Err(ListAdminContentNoticesError::CursorScopeMismatch)
            ));
            assert_eq!(0, lock(&state).searches);
        }
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures_as_transaction_failures() {
        let state = SharedState::default();
        lock(&state).fail_begin = true;
        let result = handler(&state, Some(UserRole::Admin))
            .execute(&context(Principal::User(UserId::new())), filtered_request())
            .await;
        assert!(matches!(
            result,
            Err(ListAdminContentNoticesError::BeginTransactionFailed(_))
        ));
        assert_eq!(0, lock(&state).searches);

        let state = SharedState::default();
        lock(&state).fail_commit = true;
        let result = handler(&state, Some(UserRole::Admin))
            .execute(&context(Principal::User(UserId::new())), filtered_request())
            .await;
        assert!(matches!(
            result,
            Err(ListAdminContentNoticesError::CommitTransactionFailed(_))
        ));
        assert_eq!(1, lock(&state).searches);
    }

    #[test]
    fn should_default_to_newest_first() {
        assert_eq!(
            Sort {
                sort: SortContentNoticeField::Created,
                order: SortOrder::Desc,
            },
            ListAdminContentNoticesRequest::default().effective_sort()
        );
    }

    #[test]
    fn should_bind_scope_to_normalized_sort_and_every_filter() {
        let explicit_default = ListAdminContentNoticesRequest {
            sort: Some(Sort {
                sort: SortContentNoticeField::Created,
                order: SortOrder::Desc,
            }),
            ..Default::default()
        };
        assert_eq!(
            ListAdminContentNoticesRequest::default().scope_binding(),
            explicit_default.scope_binding()
        );

        let base = filtered_request();
        let variants = [
            ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ListingSource),
                ..base.search.clone()
            },
            ContentNoticeSearch {
                listing_source_id: None,
                ..base.search.clone()
            },
            ContentNoticeSearch {
                product_listing_id: Some(ProductListingId::new()),
                ..base.search.clone()
            },
            ContentNoticeSearch {
                reason_category: None,
                ..base.search.clone()
            },
            ContentNoticeSearch {
                submitter_role: Some(SubmitterRole::Provider),
                ..base.search.clone()
            },
            ContentNoticeSearch {
                created: None,
                ..base.search.clone()
            },
        ];
        for search in variants {
            let variant = ListAdminContentNoticesRequest {
                search,
                ..base.clone()
            };
            assert_ne!(base.scope_binding(), variant.scope_binding());
        }
    }
}
