use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{
        CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
    },
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct AdministeredListingSourcesContinuation {
    pub user_id: UserId,
    pub position: AdministeredListingSourcePosition,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListAdministeredListingSourcesRequest {
    pub size: u32,
    pub after: Option<AdministeredListingSourcesContinuation>,
}
impl Default for ListAdministeredListingSourcesRequest {
    fn default() -> Self {
        Self {
            size: 50,
            after: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListAdministeredListingSourcesResult {
    pub items: Vec<AdministeredListingSource>,
    pub next: Option<AdministeredListingSourcesContinuation>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListAdministeredListingSourcesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("size must be between 1 and 100")]
    InvalidPageSize,
    #[error("continuation belongs to another actor")]
    CursorScopeMismatch,
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
        context
            .principal
            .require_credential_capability(CredentialCapability::ListingSourcesRead)
            .map_err(|error| match error {
                CredentialAuthorizationError::AuthenticationRequired(_) => {
                    ListAdministeredListingSourcesError::AuthenticatedActorRequired
                }
                CredentialAuthorizationError::InsufficientCapability { .. } => {
                    ListAdministeredListingSourcesError::Forbidden
                }
            })?;
        let user_id = match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => id,
            Principal::Anonymous => {
                return Err(ListAdministeredListingSourcesError::AuthenticatedActorRequired);
            }
            Principal::Service(_) | Principal::System => {
                return Err(ListAdministeredListingSourcesError::Forbidden);
            }
        };
        if !(1..=100).contains(&request.size) {
            return Err(ListAdministeredListingSourcesError::InvalidPageSize);
        }
        if request
            .after
            .as_ref()
            .is_some_and(|after| after.user_id != user_id)
        {
            return Err(ListAdministeredListingSourcesError::CursorScopeMismatch);
        }
        let page = self
            .authorization
            .list_sources_user_administers(
                user_id,
                request.size,
                request.after.as_ref().map(|after| &after.position),
            )
            .await?;
        Ok(ListAdministeredListingSourcesResult {
            items: page.items,
            next: page
                .next
                .map(|position| AdministeredListingSourcesContinuation { user_id, position }),
        })
    }
}
impl From<SourceAuthorizationError> for ListAdministeredListingSourcesError {
    fn from(value: SourceAuthorizationError) -> Self {
        match value {
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
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use std::{collections::BTreeSet, sync::Mutex};
    type Read = (UserId, u32, Option<AdministeredListingSourcePosition>);
    struct Authorization(Mutex<Vec<Read>>);
    #[async_trait::async_trait]
    impl ListingSourceAuthorization for Authorization {
        async fn can_write_source(
            &self,
            _: UserId,
            _: ListingSourceId,
        ) -> Result<bool, SourceAuthorizationError> {
            panic!("not a write check")
        }
        async fn list_sources_user_administers(
            &self,
            user: UserId,
            size: u32,
            after: Option<&AdministeredListingSourcePosition>,
        ) -> Result<AdministeredListingSourcePage, SourceAuthorizationError> {
            self.0.lock().unwrap().push((user, size, after.cloned()));
            Ok(AdministeredListingSourcePage {
                items: vec![],
                next: Some(position()),
            })
        }
    }
    fn position() -> AdministeredListingSourcePosition {
        AdministeredListingSourcePosition {
            name: ListingSourceName::try_from("Same name").unwrap(),
            listing_source_id: ListingSourceId::new(),
        }
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn delegated(
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    ) -> OperationContext {
        context(Principal::DelegatedUser {
            user_id,
            capabilities,
        })
    }
    #[tokio::test]
    async fn should_derive_actor_and_recheck_capability_on_every_page() {
        let user = UserId::new();
        let handler = ListAdministeredListingSourcesHandler::new(Authorization(Mutex::new(vec![])));
        let context = delegated(
            user,
            BTreeSet::from([CredentialCapability::ListingSourcesRead]),
        );
        let first = handler.execute(&context, Default::default()).await.unwrap();
        assert_eq!(user, first.next.as_ref().unwrap().user_id);
        let after = first.next.unwrap();
        handler
            .execute(
                &context,
                ListAdministeredListingSourcesRequest {
                    size: 2,
                    after: Some(after.clone()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            vec![(user, 50, None), (user, 2, Some(after.position.clone()))],
            *handler.authorization.0.lock().unwrap()
        );
        let denied = handler
            .execute(
                &delegated(user, BTreeSet::new()),
                ListAdministeredListingSourcesRequest {
                    size: 2,
                    after: Some(after),
                },
            )
            .await;
        assert!(matches!(
            denied,
            Err(ListAdministeredListingSourcesError::Forbidden)
        ));
        assert_eq!(2, handler.authorization.0.lock().unwrap().len());
    }
    #[tokio::test]
    async fn should_reject_cross_actor_cursor_invalid_sizes_and_non_users_before_reading() {
        let handler = ListAdministeredListingSourcesHandler::new(Authorization(Mutex::new(vec![])));
        let actor = context(Principal::User(UserId::new()));
        for size in [0, 101, u32::MAX] {
            assert!(matches!(
                handler
                    .execute(
                        &actor,
                        ListAdministeredListingSourcesRequest { size, after: None }
                    )
                    .await,
                Err(ListAdministeredListingSourcesError::InvalidPageSize)
            ));
        }
        let after = AdministeredListingSourcesContinuation {
            user_id: UserId::new(),
            position: position(),
        };
        assert!(matches!(
            handler
                .execute(
                    &actor,
                    ListAdministeredListingSourcesRequest {
                        size: 50,
                        after: Some(after)
                    }
                )
                .await,
            Err(ListAdministeredListingSourcesError::CursorScopeMismatch)
        ));
        for principal in [
            Principal::Anonymous,
            Principal::System,
            Principal::Service("service".into()),
        ] {
            assert!(
                handler
                    .execute(&context(principal), Default::default())
                    .await
                    .is_err()
            );
        }
        assert!(handler.authorization.0.lock().unwrap().is_empty());
    }
}
