use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientRepository, OAuthClientRepositoryFactory};
use crate::use_cases::support::{authorize_oauth_admin, validate_redirect_uris};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::change_outcome::ChangeOutcome;
use domain_primitives::versioned::Versioned;
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

pub struct UpdateOAuthClientHandler<U, C> {
    unit_of_work: U,
    clients: C,
}
impl<U, C> UpdateOAuthClientHandler<U, C> {
    pub fn new(unit_of_work: U, clients: C) -> Self {
        Self {
            unit_of_work,
            clients,
        }
    }
}

#[async_trait::async_trait]
impl<U, C> UpdateOAuthClientUseCase for UpdateOAuthClientHandler<U, C>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
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

        let mut tx = self.unit_of_work.begin().await?;
        let Versioned {
            value: mut client,
            version: loaded_version,
        } = self
            .clients
            .in_transaction(&mut tx)
            .find_by_id(*client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)?;

        let outcome = apply_client_metadata_changes(&mut client, command);
        let changed = outcome.changed();
        let client = if changed {
            self.clients
                .in_transaction(&mut tx)
                .update(&client, loaded_version)
                .await?
                .value
        } else {
            client
        };

        tx.commit().await?;
        tracing::info!(event = "oauth_client.updated", actor_id = %context.principal.label(), client_id = %client.client_id(), outcome = if changed { "changed" } else { "unchanged" });
        Ok(client)
    }
}

fn apply_client_metadata_changes(
    client: &mut OAuthClient,
    command: UpdateOAuthClientCommand,
) -> ChangeOutcome {
    let mut outcome = ChangeOutcome::Unchanged;
    if let Some(name) = command.name {
        outcome = outcome.combine(client.change_name(name));
    }
    if let Some(redirect_uris) = command.redirect_uris {
        outcome = outcome.combine(client.replace_redirect_uris(redirect_uris));
    }
    if let Some(tos_uri) = command.tos_uri {
        outcome = outcome.combine(client.change_tos_uri(tos_uri));
    }
    if let Some(policy_uri) = command.policy_uri {
        outcome = outcome.combine(client.change_policy_uri(policy_uri));
    }
    if let Some(client_uri) = command.client_uri {
        outcome = outcome.combine(client.change_client_uri(client_uri));
    }
    if let Some(logo_uri) = command.logo_uri {
        outcome = outcome.combine(client.change_logo_uri(logo_uri));
    }
    if let Some(scopes) = command.scopes {
        outcome = outcome.combine(client.replace_scopes(scopes));
    }
    outcome
}
