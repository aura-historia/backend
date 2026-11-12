use super::listing_source_references::{ReferencedListingSources, SearchFilterDetails};
use crate::ports::{SearchFilterReadError, SearchFilterReader};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use listing_source_service::use_cases::queries::get_public_listing_sources_by_ids::GetPublicListingSourcesByIdsUseCase;
use std::sync::Arc;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnedSearchFiltersRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnedSearchFiltersResult {
    pub items: Vec<SearchFilterDetails>,
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
    #[error("search filter listing source read failed")]
    ListingSourceReadFailed {
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
    listing_sources: Arc<dyn GetPublicListingSourcesByIdsUseCase>,
}

impl<R> ListOwnedSearchFiltersHandler<R> {
    pub fn new(filters: R, listing_sources: Arc<dyn GetPublicListingSourcesByIdsUseCase>) -> Self {
        Self {
            filters,
            listing_sources,
        }
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
        let views = self
            .filters
            .find_for_user(request.user_id)
            .await
            .map_err(read_error)?;
        let listing_sources = ReferencedListingSources::resolve(
            self.listing_sources.as_ref(),
            context,
            views.iter().map(|view| &view.search),
        )
        .await
        .map_err(
            |error| ListOwnedSearchFiltersError::ListingSourceReadFailed {
                source: box_error(error),
            },
        )?;
        Ok(ListOwnedSearchFiltersResult {
            items: views
                .into_iter()
                .map(|view| listing_sources.details(view))
                .collect(),
        })
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListOwnedSearchFiltersError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersRead)
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
