use crate::ports::{
    SearchFilterMatchListQuery, SearchFilterMatchReadError, SearchFilterMatchReader,
    SearchFilterMatchView,
};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::sort::SortOrder;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListSearchFilterMatchesRequest {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub cursor: Option<Cursor<OffsetDateTime>>,
    pub order: SortOrder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListSearchFilterMatchesResult {
    pub matches: CursoredResult<SearchFilterMatchView, OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListSearchFilterMatchesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter match read failed")]
    SearchFilterMatchReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListSearchFilterMatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError>;
}

pub struct ListSearchFilterMatchesHandler<R> {
    matches: R,
}

impl<R> ListSearchFilterMatchesHandler<R> {
    pub fn new(matches: R) -> Self {
        Self { matches }
    }
}

#[async_trait::async_trait]
impl<R> ListSearchFilterMatchesUseCase for ListSearchFilterMatchesHandler<R>
where
    R: SearchFilterMatchReader,
{
    #[tracing::instrument(
        name = "list_search_filter_matches",
        skip_all,
        fields(
            search_filter_id = %request.search_filter_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
        authorize_owner(context, request.user_id)?;
        let matches = self
            .matches
            .list_for_owned_filter(&SearchFilterMatchListQuery {
                user_id: request.user_id,
                search_filter_id: request.search_filter_id,
                cursor: request.cursor,
                order: request.order,
            })
            .await
            .map_err(read_error)?
            .ok_or(ListSearchFilterMatchesError::SearchFilterNotFound)?;
        Ok(ListSearchFilterMatchesResult { matches })
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListSearchFilterMatchesError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListSearchFilterMatchesError>()
}

fn read_error(error: SearchFilterMatchReadError) -> ListSearchFilterMatchesError {
    ListSearchFilterMatchesError::SearchFilterMatchReadFailed {
        source: box_error(error),
    }
}

impl From<OperationAuthorizationError> for ListSearchFilterMatchesError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                Self::ActorMayNotManageSearchFilter
            }
        }
    }
}
