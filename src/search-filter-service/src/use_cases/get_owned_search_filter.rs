use crate::ports::{SearchFilterReadError, SearchFilterReader, SearchFilterView};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct GetOwnedSearchFilterRequest {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetOwnedSearchFilterResult {
    pub filter: SearchFilterView,
}

#[derive(Debug, thiserror::Error)]
pub enum GetOwnedSearchFilterError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter read failed")]
    SearchFilterReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetOwnedSearchFilterUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnedSearchFilterRequest,
    ) -> Result<GetOwnedSearchFilterResult, GetOwnedSearchFilterError>;
}

pub struct GetOwnedSearchFilterHandler<R> {
    filters: R,
}

impl<R> GetOwnedSearchFilterHandler<R> {
    pub fn new(filters: R) -> Self {
        Self { filters }
    }
}

#[async_trait::async_trait]
impl<R> GetOwnedSearchFilterUseCase for GetOwnedSearchFilterHandler<R>
where
    R: SearchFilterReader,
{
    #[tracing::instrument(
        name = "get_owned_search_filter",
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
        request: GetOwnedSearchFilterRequest,
    ) -> Result<GetOwnedSearchFilterResult, GetOwnedSearchFilterError> {
        authorize_owner(context, request.user_id)?;
        let filter = self
            .filters
            .find_for_user_by_id(request.user_id, request.search_filter_id)
            .await
            .map_err(read_error)?
            .ok_or(GetOwnedSearchFilterError::SearchFilterNotFound)?;
        Ok(GetOwnedSearchFilterResult { filter })
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), GetOwnedSearchFilterError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersRead)
        .user(&user_id)
        .service_or_system()
        .authorize::<GetOwnedSearchFilterError>()
}

fn read_error(error: SearchFilterReadError) -> GetOwnedSearchFilterError {
    GetOwnedSearchFilterError::SearchFilterReadFailed {
        source: box_error(error),
    }
}

impl From<OperationAuthorizationError> for GetOwnedSearchFilterError {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_require_read_scope_and_ownership_for_search_filter_reads() {
        let user_id = UserId::new();
        for (capabilities, owner, allowed) in [
            (std::collections::BTreeSet::new(), user_id, false),
            (
                std::collections::BTreeSet::from([CredentialCapability::SearchFiltersWrite]),
                user_id,
                false,
            ),
            (
                std::collections::BTreeSet::from([CredentialCapability::SearchFiltersRead]),
                UserId::new(),
                false,
            ),
            (
                std::collections::BTreeSet::from([CredentialCapability::SearchFiltersRead]),
                user_id,
                true,
            ),
        ] {
            let context = OperationContext {
                principal: application::operation_context::Principal::DelegatedUser {
                    user_id,
                    capabilities,
                },
                request_id: application::operation_context::RequestId::new("scope-test"),
                correlation_id: application::operation_context::CorrelationId::new("scope-test"),
            };
            assert_eq!(allowed, authorize_owner(&context, owner).is_ok());
        }
    }
}
