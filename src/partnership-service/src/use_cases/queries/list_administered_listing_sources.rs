use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
};
use user_core::user_id::UserId;
#[derive(Debug, Clone, PartialEq)]
pub struct ListAdministeredListingSourcesRequest {
    pub user_id: UserId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListAdministeredListingSourcesResult {
    pub items: Vec<AdministeredListingSource>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListAdministeredListingSourcesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait ListAdministeredListingSourcesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdministeredListingSourcesRequest,
    ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError>;
}
pub struct ListAdministeredListingSourcesHandler<A> {
    authorization: A,
}
impl<A> ListAdministeredListingSourcesHandler<A> {
    pub fn new(authorization: A) -> Self {
        Self { authorization }
    }
}
#[async_trait::async_trait]
impl<A: ListingSourceAuthorization> ListAdministeredListingSourcesUseCase
    for ListAdministeredListingSourcesHandler<A>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdministeredListingSourcesRequest,
    ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError> {
        match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. }
                if id == request.user_id => {}
            Principal::Service(_) | Principal::System => {}
            Principal::Anonymous => {
                return Err(ListAdministeredListingSourcesError::AuthenticatedActorRequired);
            }
            _ => return Err(ListAdministeredListingSourcesError::Forbidden),
        };
        Ok(ListAdministeredListingSourcesResult {
            items: self
                .authorization
                .list_sources_user_administers(request.user_id)
                .await?,
        })
    }
}
impl From<SourceAuthorizationError> for ListAdministeredListingSourcesError {
    fn from(v: SourceAuthorizationError) -> Self {
        match v {
            SourceAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            SourceAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            SourceAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, RequestId};

    struct Authorization;

    #[async_trait::async_trait]
    impl ListingSourceAuthorization for Authorization {
        async fn can_write_source(
            &self,
            _user_id: UserId,
            _listing_source_id: listing_source_core::ListingSourceId,
        ) -> Result<bool, SourceAuthorizationError> {
            Ok(false)
        }

        async fn list_sources_user_administers(
            &self,
            _user_id: UserId,
        ) -> Result<Vec<AdministeredListingSource>, SourceAuthorizationError> {
            Ok(Vec::new())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[tokio::test]
    async fn should_list_sources_only_for_the_requested_user() {
        let user_id = UserId::new();
        let result = ListAdministeredListingSourcesHandler::new(Authorization)
            .execute(
                &context(Principal::User(user_id)),
                ListAdministeredListingSourcesRequest { user_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(ListAdministeredListingSourcesResult { items }) if items.is_empty()
        ));
    }

    #[tokio::test]
    async fn should_reject_listing_sources_for_another_user() {
        let result = ListAdministeredListingSourcesHandler::new(Authorization)
            .execute(
                &context(Principal::User(UserId::new())),
                ListAdministeredListingSourcesRequest {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(ListAdministeredListingSourcesError::Forbidden)
        ));
    }
}
