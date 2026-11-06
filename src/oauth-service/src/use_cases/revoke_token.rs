use crate::error::OAuthServiceError;
use crate::ports::{
    OAuthClientRepositoryFactory, OAuthConnectionRepository, OAuthConnectionRepositoryFactory,
    RefreshTokenFamilyRepository, RefreshTokenFamilyRepositoryFactory,
};
use crate::use_cases::metadata_document_client_credentials::MetadataDocumentClientCredentials;
use crate::use_cases::support::authenticate_client;
use crate::use_cases::token_by_authorization_code::OAuthTokenClientAuthentication;
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_core::refresh_token::{HashedRawRefreshToken, RawRefreshToken};
use user_core::access_token::{AccessTokenOrigin, HashedRawAccessToken, RawAccessToken};
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory};

/// A token a client asks to revoke (RFC 7009).
#[derive(Debug, Clone, PartialEq)]
pub enum RevocableToken {
    AccessToken(RawAccessToken),
    /// Revoking a refresh token ends the whole connection it belongs to.
    RefreshToken(RawRefreshToken),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevokeTokenRequest {
    pub token: RevocableToken,
    pub client: OAuthTokenClientAuthentication,
}

#[async_trait::async_trait]
pub trait RevokeTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError>;
}

/// The persistence a revocation reads and deletes.
pub struct RevocationStores<C, R, N, F> {
    pub clients: C,
    pub access_tokens: R,
    pub connections: N,
    pub refresh_token_families: F,
}

pub struct RevokeTokenHandler<U, C, R, N, F> {
    unit_of_work: U,
    stores: RevocationStores<C, R, N, F>,
    metadata_document_clients: MetadataDocumentClientCredentials,
}
impl<U, C, R, N, F> RevokeTokenHandler<U, C, R, N, F> {
    pub fn new(
        unit_of_work: U,
        stores: RevocationStores<C, R, N, F>,
        metadata_document_clients: MetadataDocumentClientCredentials,
    ) -> Self {
        Self {
            unit_of_work,
            stores,
            metadata_document_clients,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, R, N, F> RevokeTokenUseCase for RevokeTokenHandler<U, C, R, N, F>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        _context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError> {
        if let OAuthTokenClientAuthentication::None { client_id } = &request.client {
            self.metadata_document_clients.admit(client_id)?;
        }
        let mut tx = self.unit_of_work.begin().await?;
        if let OAuthTokenClientAuthentication::ClientSecretPost {
            client_id,
            client_secret,
        } = &request.client
        {
            authenticate_client(
                &mut self.stores.clients.in_transaction(&mut tx),
                client_id,
                client_secret,
            )
            .await?;
        }
        let client_id = request.client.client_id();
        match request.token {
            RevocableToken::AccessToken(token) => {
                self.revoke_access_token(&mut tx, &client_id, token).await?
            }
            RevocableToken::RefreshToken(token) => {
                self.revoke_connection(&mut tx, &client_id, token).await?
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

impl<U, C, R, N, F> RevokeTokenHandler<U, C, R, N, F>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
{
    /// An unknown token is already revoked; another client's token is refused.
    async fn revoke_access_token(
        &self,
        tx: &mut U::Tx,
        client_id: &OAuthClientIdentifier,
        token: RawAccessToken,
    ) -> Result<(), OAuthServiceError> {
        let mut access_tokens = self.stores.access_tokens.in_transaction(tx);
        let Some(token) = access_tokens
            .find_by_hashed_token(&HashedRawAccessToken::from(token))
            .await?
        else {
            return Ok(());
        };
        if !issued_to(token.value.origin(), client_id) {
            return Err(OAuthServiceError::Forbidden);
        }
        access_tokens
            .delete_by_id(token.value.user_id(), token.value.id())
            .await?;
        Ok(())
    }

    /// Ends the connection of the family that issued the refresh token.
    async fn revoke_connection(
        &self,
        tx: &mut U::Tx,
        client_id: &OAuthClientIdentifier,
        token: RawRefreshToken,
    ) -> Result<(), OAuthServiceError> {
        let token = HashedRawRefreshToken::from(token);
        let Some(connection_id) = self
            .stores
            .refresh_token_families
            .in_transaction(tx)
            .find_connection_id(&token)
            .await?
        else {
            return Ok(());
        };
        let mut connections = self.stores.connections.in_transaction(tx);
        let Some(connection) = connections.find_by_id_for_update(connection_id).await? else {
            return Ok(());
        };
        if !matches!(client_id, OAuthClientIdentifier::MetadataDocument(client_id) if client_id == connection.client_id())
        {
            return Err(OAuthServiceError::Forbidden);
        }
        connections
            .delete(connection.user_id(), connection.id())
            .await?;
        tracing::info!(
            event = "oauth_connection.revoked",
            user_id = %connection.user_id(),
            connection_id = %connection.id(),
            client_host = connection.client_id().host(),
            outcome = "success",
        );
        Ok(())
    }
}

fn issued_to(origin: &AccessTokenOrigin, client_id: &OAuthClientIdentifier) -> bool {
    match (origin, client_id) {
        (AccessTokenOrigin::OAuth { client_id: issued }, OAuthClientIdentifier::Registered(id)) => {
            issued == id
        }
        (
            AccessTokenOrigin::OAuthMetadataDocument {
                client_id: issued, ..
            },
            OAuthClientIdentifier::MetadataDocument(id),
        ) => issued == id,
        _ => false,
    }
}
