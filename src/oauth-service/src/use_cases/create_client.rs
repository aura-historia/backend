use crate::error::OAuthServiceError;
use crate::ports::OAuthClientRepository;
use crate::use_cases::support::{authorize_oauth_admin, validate_redirect_uris};
use application::operation_context::OperationContext;
use common::oauth_client_id::OAuthClientId;
use oauth_core::client::{OAuthClient, OAuthClientName};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::{HashedRawOAuthClientSecret, RawOAuthClientSecret, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOAuthClientCommand {
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOAuthClientResult {
    pub raw_client_secret: RawOAuthClientSecret,
    pub client: OAuthClient,
}

#[async_trait::async_trait]
pub trait CreateOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateOAuthClientCommand,
    ) -> Result<CreateOAuthClientResult, OAuthServiceError>;
}

pub struct CreateOAuthClientHandler<R> {
    repository: R,
}
impl<R> CreateOAuthClientHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<R> CreateOAuthClientUseCase for CreateOAuthClientHandler<R>
where
    R: OAuthClientRepository,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateOAuthClientCommand,
    ) -> Result<CreateOAuthClientResult, OAuthServiceError> {
        authorize_oauth_admin(context)?;
        validate_redirect_uris(&command.redirect_uris)
            .map_err(OAuthServiceError::InvalidClientMetadata)?;
        let now = OffsetDateTime::now_utc();
        let raw_client_secret = RawOAuthClientSecret::new();
        let client = OAuthClient {
            client_id: OAuthClientId::new(),
            hashed_client_secret: HashedRawOAuthClientSecret::from(raw_client_secret.clone()),
            name: command.name,
            redirect_uris: command.redirect_uris,
            tos_uri: command.tos_uri,
            policy_uri: command.policy_uri,
            client_uri: command.client_uri,
            logo_uri: command.logo_uri,
            scopes: command.scopes,
            created: now,
            updated: now,
        };
        self.repository
            .insert(client.clone(), raw_client_secret.clone())
            .await?;
        tracing::info!(event = "oauth_client.created", actor_id = %context.principal.label(), client_id = %client.client_id, outcome = "success");
        Ok(CreateOAuthClientResult {
            raw_client_secret,
            client,
        })
    }
}
