use crate::ports::{SearchFilterReadError, SearchFilterReader, SearchFilterView};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnedSearchFiltersRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnedSearchFiltersResult {
    pub items: Vec<SearchFilterView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListOwnedSearchFiltersError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter list read failed")]
    SearchFilterListReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListOwnedSearchFiltersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnedSearchFiltersRequest,
    ) -> Result<ListOwnedSearchFiltersResult, ListOwnedSearchFiltersError>;
}

pub struct ListOwnedSearchFiltersHandler<R> {
    filters: R,
}

impl<R> ListOwnedSearchFiltersHandler<R> {
    pub fn new(filters: R) -> Self {
        Self { filters }
    }
}

#[async_trait::async_trait]
impl<R> ListOwnedSearchFiltersUseCase for ListOwnedSearchFiltersHandler<R>
where
    R: SearchFilterReader,
{
    #[tracing::instrument(
        name = "list_owned_search_filters",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnedSearchFiltersRequest,
    ) -> Result<ListOwnedSearchFiltersResult, ListOwnedSearchFiltersError> {
        authorize_owner(context, request.user_id)?;
        let items = self
            .filters
            .find_for_user(request.user_id)
            .await
            .map_err(read_error)?;
        Ok(ListOwnedSearchFiltersResult { items })
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListOwnedSearchFiltersError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListOwnedSearchFiltersError>()
}

fn read_error(error: SearchFilterReadError) -> ListOwnedSearchFiltersError {
    ListOwnedSearchFiltersError::SearchFilterListReadFailed {
        source: box_error(error),
    }
}

impl From<OperationAuthorizationError> for ListOwnedSearchFiltersError {
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
