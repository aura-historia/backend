use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientRepository, OAuthClientRepositoryFactory};
use crate::use_cases::support::{authorize_oauth_admin, validate_redirect_uris};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::{OAuthClient, OAuthClientName, RehydratedOAuthClientState};
use std::collections::HashSet;
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

pub struct CreateOAuthClientHandler<U, C> {
    unit_of_work: U,
    clients: C,
}
impl<U, C> CreateOAuthClientHandler<U, C> {
    pub fn new(unit_of_work: U, clients: C) -> Self {
        Self {
            unit_of_work,
            clients,
        }
    }
}

#[async_trait::async_trait]
impl<U, C> CreateOAuthClientUseCase for CreateOAuthClientHandler<U, C>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateOAuthClientCommand,
    ) -> Result<CreateOAuthClientResult, OAuthServiceError> {
        authorize_oauth_admin(context)?;
        validate_redirect_uris(&command.redirect_uris)
            .map_err(OAuthServiceError::InvalidClientMetadata)?;

        let raw_client_secret = RawOAuthClientSecret::new();
        let client = OAuthClient::create(RehydratedOAuthClientState {
            client_id: OAuthClientId::new(),
            hashed_client_secret: HashedRawOAuthClientSecret::from(raw_client_secret.clone()),
            name: command.name,
            redirect_uris: command.redirect_uris,
            tos_uri: command.tos_uri,
            policy_uri: command.policy_uri,
            client_uri: command.client_uri,
            logo_uri: command.logo_uri,
            scopes: command.scopes,
        });

        let mut tx = self.unit_of_work.begin().await?;
        let persisted = self.clients.in_transaction(&mut tx).insert(&client).await?;
        tx.commit().await?;

        tracing::info!(event = "oauth_client.created", actor_id = %context.principal.label(), client_id = %persisted.value.client_id(), outcome = "success");
        Ok(CreateOAuthClientResult {
            raw_client_secret,
            client: persisted.value,
        })
    }
}
