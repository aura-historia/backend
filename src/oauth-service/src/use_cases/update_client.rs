use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientPatch, OAuthClientRepository};
use crate::use_cases::support::{authorize_oauth_admin, validate_redirect_uris};
use application::operation_context::OperationContext;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::{OAuthClient, OAuthClientName};
use std::collections::HashSet;
use url::Url;
use user_core::access_token::Scope;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOAuthClientCommand {
    pub name: Option<OAuthClientName>,
    pub redirect_uris: Option<HashSet<Url>>,
    pub tos_uri: Option<Url>,
    pub policy_uri: Option<Url>,
    pub client_uri: Option<Url>,
    pub logo_uri: Option<Url>,
    pub scopes: Option<HashSet<Scope>>,
}

#[async_trait::async_trait]
pub trait UpdateOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClient, OAuthServiceError>;
}

pub struct UpdateOAuthClientHandler<R> {
    repository: R,
}
impl<R> UpdateOAuthClientHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}
#[async_trait::async_trait]
impl<R> UpdateOAuthClientUseCase for UpdateOAuthClientHandler<R>
where
    R: OAuthClientRepository,
{
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClient, OAuthServiceError> {
        authorize_oauth_admin(context)?;
        if let Some(redirect_uris) = &command.redirect_uris {
            validate_redirect_uris(redirect_uris)
                .map_err(OAuthServiceError::InvalidClientMetadata)?;
        }
        let patch = OAuthClientPatch {
            name: command.name,
            redirect_uris: command.redirect_uris,
            tos_uri: command.tos_uri,
            policy_uri: command.policy_uri,
            client_uri: command.client_uri,
            logo_uri: command.logo_uri,
            scopes: command.scopes,

            updated: time::OffsetDateTime::now_utc(),
        };
        self.repository
            .update(client_id, patch)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)
    }
}
