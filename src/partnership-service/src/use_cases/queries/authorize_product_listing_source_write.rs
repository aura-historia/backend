use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
};
use listing_source_core::ListingSourceId;
use user_core::user_id::UserId;
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeProductListingSourceWriteRequest {
    pub listing_source_id: ListingSourceId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizeProductListingSourceWriteResult {
    pub authorized: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum AuthorizeProductListingSourceWriteError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary source authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid source authorization data")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal source authorization failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait AuthorizeProductListingSourceWriteUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeProductListingSourceWriteRequest,
    ) -> Result<AuthorizeProductListingSourceWriteResult, AuthorizeProductListingSourceWriteError>;
}
pub struct AuthorizeProductListingSourceWriteHandler<A> {
    authorization: A,
}
impl<A> AuthorizeProductListingSourceWriteHandler<A> {
    pub fn new(authorization: A) -> Self {
        Self { authorization }
    }
}
#[async_trait::async_trait]
impl<A: ListingSourceAuthorization> AuthorizeProductListingSourceWriteUseCase
    for AuthorizeProductListingSourceWriteHandler<A>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeProductListingSourceWriteRequest,
    ) -> Result<AuthorizeProductListingSourceWriteResult, AuthorizeProductListingSourceWriteError>
    {
        let user = actor(context)?;
        Ok(AuthorizeProductListingSourceWriteResult {
            authorized: self
                .authorization
                .can_write_source(user, request.listing_source_id)
                .await?,
        })
    }
}
fn actor(context: &OperationContext) -> Result<UserId, AuthorizeProductListingSourceWriteError> {
    context
        .require()
        .credential_capability(CredentialCapability::ProductListingsWrite)
        .any_user()
        .authorize::<OperationAuthorizationError>()
        .map_err(|error| match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                AuthorizeProductListingSourceWriteError::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                AuthorizeProductListingSourceWriteError::Forbidden
            }
        })?;
    match context.principal {
        Principal::User(user) | Principal::DelegatedUser { user_id: user, .. } => Ok(user),
        _ => Err(AuthorizeProductListingSourceWriteError::Forbidden),
    }
}
impl From<SourceAuthorizationError> for AuthorizeProductListingSourceWriteError {
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

    struct Authorization {
        allowed: bool,
    }

    #[async_trait::async_trait]
    impl ListingSourceAuthorization for Authorization {
        async fn can_write_source(
            &self,
            _user_id: UserId,
            _listing_source_id: ListingSourceId,
        ) -> Result<bool, SourceAuthorizationError> {
            Ok(self.allowed)
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
    async fn should_return_authorization_for_authenticated_user_source_write() {
        let user_id = UserId::new();
        let result =
            AuthorizeProductListingSourceWriteHandler::new(Authorization { allowed: true })
                .execute(
                    &context(Principal::User(user_id)),
                    AuthorizeProductListingSourceWriteRequest {
                        listing_source_id: ListingSourceId::new(),
                    },
                )
                .await;

        assert!(matches!(
            result,
            Ok(AuthorizeProductListingSourceWriteResult { authorized: true })
        ));
    }

    #[tokio::test]
    async fn should_reject_anonymous_source_write_authorization() {
        let result =
            AuthorizeProductListingSourceWriteHandler::new(Authorization { allowed: true })
                .execute(
                    &context(Principal::Anonymous),
                    AuthorizeProductListingSourceWriteRequest {
                        listing_source_id: ListingSourceId::new(),
                    },
                )
                .await;

        assert!(matches!(
            result,
            Err(AuthorizeProductListingSourceWriteError::AuthenticatedActorRequired)
        ));
    }
}
