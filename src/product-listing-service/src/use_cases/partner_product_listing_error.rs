use super::admin_product_listing_error::AdminProductListingError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use listing_source_core::ListingSourceId;
use partnership_service::ports::{ListingSourceAuthorization, SourceAuthorizationError};

#[derive(Debug, thiserror::Error)]
pub enum PartnerProductListingError {
    #[error("authenticated user required")]
    AuthenticatedActorRequired,
    #[error("ListingSource Partnership authority and credential capability required")]
    Forbidden,
    #[error(transparent)]
    Management(#[from] AdminProductListingError),
}

pub(crate) async fn authorize_partner<A: ListingSourceAuthorization>(
    context: &OperationContext,
    authorization: &A,
    source: ListingSourceId,
    capability: CredentialCapability,
) -> Result<(), PartnerProductListingError> {
    context
        .require()
        .credential_capability(capability)
        .any_user()
        .authorize::<OperationAuthorizationError>()
        .map_err(|error| match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                PartnerProductListingError::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                PartnerProductListingError::Forbidden
            }
        })?;
    let user = match context.principal {
        Principal::User(user) | Principal::DelegatedUser { user_id: user, .. } => user,
        _ => return Err(PartnerProductListingError::Forbidden),
    };
    let allowed =
        authorization
            .can_write_source(user, source)
            .await
            .map_err(|error| match error {
                SourceAuthorizationError::TemporarilyUnavailable { source } => {
                    AdminProductListingError::TemporarilyUnavailable { source }
                }
                SourceAuthorizationError::InvalidReadModel { source } => {
                    AdminProductListingError::InvalidPersistedState { source }
                }
                SourceAuthorizationError::Internal { source } => {
                    AdminProductListingError::Internal { source }
                }
            })?;
    if allowed {
        Ok(())
    } else {
        Err(PartnerProductListingError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, RequestId},
    };
    use partnership_service::ports::AdministeredListingSource;
    use std::{
        collections::BTreeSet,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use user_core::user_id::UserId;

    struct Authorization {
        calls: AtomicUsize,
        failure: bool,
    }
    #[async_trait::async_trait]
    impl ListingSourceAuthorization for Authorization {
        async fn can_write_source(
            &self,
            _: UserId,
            _: ListingSourceId,
        ) -> Result<bool, SourceAuthorizationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.failure {
                Err(SourceAuthorizationError::TemporarilyUnavailable {
                    source: static_error("authorization dependency"),
                })
            } else {
                Ok(true)
            }
        }
        async fn list_sources_user_administers(
            &self,
            _: UserId,
        ) -> Result<Vec<AdministeredListingSource>, SourceAuthorizationError> {
            panic!("management checks only the path source")
        }
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            command_key: None,
            principal,
            request_id: RequestId::new("partner"),
            correlation_id: CorrelationId::new("partner"),
        }
    }
    #[tokio::test]
    async fn rejects_non_user_principals_and_missing_capability_before_source_dependency() {
        let authorization = Authorization {
            calls: AtomicUsize::new(0),
            failure: false,
        };
        for capability in [
            CredentialCapability::ProductListingsRead,
            CredentialCapability::ProductListingsWrite,
        ] {
            for principal in [
                Principal::Anonymous,
                Principal::System,
                Principal::Service("ingestion".to_owned()),
                Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: BTreeSet::new(),
                },
            ] {
                let result = authorize_partner(
                    &context(principal),
                    &authorization,
                    ListingSourceId::new(),
                    capability,
                )
                .await;
                assert!(matches!(
                    result,
                    Err(PartnerProductListingError::AuthenticatedActorRequired
                        | PartnerProductListingError::Forbidden)
                ));
                assert_eq!(0, authorization.calls.load(Ordering::SeqCst));
            }
        }
    }
    #[tokio::test]
    async fn preserves_authorization_failure_cause() {
        let authorization = Authorization {
            calls: AtomicUsize::new(0),
            failure: true,
        };
        let result = authorize_partner(
            &context(Principal::User(UserId::new())),
            &authorization,
            ListingSourceId::new(),
            CredentialCapability::ProductListingsRead,
        )
        .await;
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            PartnerProductListingError::Management(
                AdminProductListingError::TemporarilyUnavailable { .. }
            )
        ));
        assert!(std::error::Error::source(&error).is_some());
    }
}
