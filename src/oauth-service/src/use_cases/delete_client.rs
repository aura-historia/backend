use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientRepository, OAuthClientRepositoryFactory};
use crate::use_cases::support::authorize_oauth_admin;
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOAuthClientResult {
    pub client_id: OAuthClientId,
}

#[async_trait::async_trait]
pub trait DeleteOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError>;
}

pub struct DeleteOAuthClientHandler<U, C> {
    unit_of_work: U,
    clients: C,
}
impl<U, C> DeleteOAuthClientHandler<U, C> {
    pub fn new(unit_of_work: U, clients: C) -> Self {
        Self {
            unit_of_work,
            clients,
        }
    }
}

#[async_trait::async_trait]
impl<U, C> DeleteOAuthClientUseCase for DeleteOAuthClientHandler<U, C>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError> {
        authorize_oauth_admin(context)?;
        let mut tx = self.unit_of_work.begin().await?;
        let deleted = self
            .clients
            .in_transaction(&mut tx)
            .delete_by_id(*client_id)
            .await?;
        if !deleted {
            return Err(OAuthServiceError::ClientNotFound);
        }
        tx.commit().await?;

        tracing::info!(event = "oauth_client.deleted", actor_id = %context.principal.label(), client_id = %client_id, outcome = "success");
        Ok(DeleteOAuthClientResult {
            client_id: *client_id,
        })
    }
}
