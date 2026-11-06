use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, OAuthConnectionRepository,
    OAuthConnectionRepositoryFactory,
};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use oauth_core::client_identifier::OAuthClientIdentifier;
use user_core::user_id::UserId;
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory, OAuthTokenHolder};

/// Revokes every credential the user granted one OAuth client.
#[derive(Debug, Clone, PartialEq)]
pub struct DisconnectOAuthClientCommand {
    pub user_id: UserId,
    pub client_id: OAuthClientIdentifier,
}

#[async_trait::async_trait]
pub trait DisconnectOAuthClientUseCase: Send + Sync {
    /// Idempotent: a client the user is not connected to is already disconnected.
    async fn execute(
        &self,
        context: &OperationContext,
        command: DisconnectOAuthClientCommand,
    ) -> Result<(), OAuthServiceError>;
}

pub struct DisconnectOAuthClientHandler<U, A, N, R> {
    unit_of_work: U,
    codes: A,
    connections: N,
    access_tokens: R,
}

impl<U, A, N, R> DisconnectOAuthClientHandler<U, A, N, R> {
    pub fn new(unit_of_work: U, codes: A, connections: N, access_tokens: R) -> Self {
        Self {
            unit_of_work,
            codes,
            connections,
            access_tokens,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, N, R> DisconnectOAuthClientUseCase for DisconnectOAuthClientHandler<U, A, N, R>
where
    U: UnitOfWork,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "disconnect_oauth_client",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DisconnectOAuthClientCommand,
    ) -> Result<(), OAuthServiceError> {
        authorize_disconnect(context, command.user_id)?;
        let mut tx = self.unit_of_work.begin().await?;
        // An approval the client has not exchanged yet must not outlive the disconnect.
        self.codes
            .in_transaction(&mut tx)
            .delete_for_user_and_client(command.user_id, &command.client_id)
            .await?;
        let revoked = match &command.client_id {
            OAuthClientIdentifier::Registered(client_id) => {
                self.access_tokens
                    .in_transaction(&mut tx)
                    .delete_by_oauth_holder(
                        command.user_id,
                        OAuthTokenHolder::RegisteredClient(*client_id),
                    )
                    .await?
                    > 0
            }
            OAuthClientIdentifier::MetadataDocument(client_id) => {
                let mut connections = self.connections.in_transaction(&mut tx);
                match connections
                    .find_by_user_and_client_for_update(command.user_id, client_id)
                    .await?
                {
                    Some(connection) => {
                        connections.delete(command.user_id, connection.id()).await?
                    }
                    None => false,
                }
            }
        };
        tx.commit().await?;
        tracing::info!(
            event = "oauth_client.disconnected",
            user_id = %command.user_id,
            revoked,
            outcome = "success",
        );
        Ok(())
    }
}

/// Like issuing an access token, disconnecting needs the user's own first-party session: a
/// delegated credential must not end the grants of other clients, or its own.
fn authorize_disconnect(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), OAuthServiceError> {
    match &context.principal {
        Principal::User(actor) if *actor == user_id => Ok(()),
        Principal::DelegatedUser { .. } => {
            let error = OAuthServiceError::DelegatedDisconnectForbidden;
            tracing::warn!(
                event = "oauth_client.disconnected",
                actor_id = %context.principal.label(),
                error_category = %error,
                outcome = "failure",
            );
            Err(error)
        }
        Principal::Anonymous => Err(OAuthServiceError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::Service(_) | Principal::System => {
            Err(OAuthServiceError::Forbidden)
        }
    }
}
