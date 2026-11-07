use crate::error::OAuthServiceError;
use crate::ports::{
    OAuthConnectionRepository, OAuthConnectionRepositoryFactory, RefreshTokenFamilyRepository,
    RefreshTokenFamilyRepositoryFactory,
};
use crate::use_cases::metadata_document_client_credentials::MetadataDocumentClientCredentials;
use crate::use_cases::token_by_authorization_code::TokenResponse;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_core::connection::OAuthConnection;
use oauth_core::refresh_token::{
    HashedRawRefreshToken, RawRefreshToken, RefreshTokenAssessment, RefreshTokenFamily,
};
use oauth_core::resource::AuraResourcePolicy;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::Scope;
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory, OAuthTokenHolder};

/// `grant_type=refresh_token` from a metadata document client, which authenticates with its
/// `client_id` alone.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenByRefreshTokenRequest {
    pub refresh_token: RawRefreshToken,
    pub client_id: MetadataDocumentClientId,
    /// A subset of the family's scopes for the new access token; all of them when absent.
    pub scope: Option<HashSet<Scope>>,
    pub resource: Option<String>,
}

#[async_trait::async_trait]
pub trait TokenByRefreshTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        request: TokenByRefreshTokenRequest,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

pub struct TokenByRefreshTokenHandler<U, N, F, R> {
    unit_of_work: U,
    connections: N,
    families: F,
    access_tokens: R,
    metadata_document_clients: MetadataDocumentClientCredentials,
    resource_policy: AuraResourcePolicy,
}

impl<U, N, F, R> TokenByRefreshTokenHandler<U, N, F, R> {
    pub fn new(
        unit_of_work: U,
        connections: N,
        families: F,
        access_tokens: R,
        metadata_document_clients: MetadataDocumentClientCredentials,
        resource_policy: AuraResourcePolicy,
    ) -> Self {
        Self {
            unit_of_work,
            connections,
            families,
            access_tokens,
            metadata_document_clients,
            resource_policy,
        }
    }
}

#[async_trait::async_trait]
impl<U, N, F, R> TokenByRefreshTokenUseCase for TokenByRefreshTokenHandler<U, N, F, R>
where
    U: UnitOfWork,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "refresh_oauth_token",
        skip_all,
        fields(client_host = request.client_id.host(), outcome = tracing::field::Empty)
    )]
    async fn execute(
        &self,
        request: TokenByRefreshTokenRequest,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let requested_resource = request
            .resource
            .as_deref()
            .map(|resource| self.resource_policy.resolve(Some(resource)))
            .transpose()
            .map_err(|_| OAuthServiceError::InvalidResource)?;
        self.metadata_document_clients.admit(&request.client_id)?;
        let token = HashedRawRefreshToken::from(request.refresh_token.clone());
        let mut tx = self.unit_of_work.begin().await?;
        let (mut connection, mut family, presented) = self
            .find_presented(&mut tx, &token, &request.client_id)
            .await?;
        let now = OffsetDateTime::now_utc();
        let result = match family.assess(presented, now) {
            RefreshTokenAssessment::Rotate => {
                if requested_resource.is_some_and(|resource| resource != family.resource()) {
                    return Err(OAuthServiceError::RefreshTokenResourceMismatch);
                }
                let scopes = match request.scope {
                    Some(scopes) if !scopes.is_subset(family.scopes()) => {
                        return Err(OAuthServiceError::InvalidScope);
                    }
                    Some(scopes) => scopes,
                    None => family.scopes().clone(),
                };
                self.rotate(&mut tx, &mut connection, &mut family, scopes, now)
                    .await
            }
            RefreshTokenAssessment::Reused => {
                self.revoke_for_reuse(&mut tx, &connection, &family).await?;
                tx.commit().await?;
                return Err(OAuthServiceError::RefreshTokenReused);
            }
            RefreshTokenAssessment::ConcurrentRefresh => {
                Err(OAuthServiceError::RefreshTokenReplaced)
            }
            RefreshTokenAssessment::Expired => Err(OAuthServiceError::RefreshTokenExpired),
        };
        let response = result?;
        tx.commit().await?;
        tracing::Span::current().record("outcome", "success");
        Ok(response)
    }
}

impl<U, N, F, R> TokenByRefreshTokenHandler<U, N, F, R>
where
    U: UnitOfWork,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    /// Locks the token's connection, then its family, the order every connection write uses.
    /// A token of another client is unknown to this one, and proves nothing about its family.
    async fn find_presented(
        &self,
        tx: &mut U::Tx,
        token: &HashedRawRefreshToken,
        client_id: &MetadataDocumentClientId,
    ) -> Result<
        (
            OAuthConnection,
            RefreshTokenFamily,
            oauth_core::refresh_token::PresentedRefreshToken,
        ),
        OAuthServiceError,
    > {
        let connection_id = self
            .families
            .in_transaction(tx)
            .find_connection_id(token)
            .await?
            .ok_or(OAuthServiceError::RefreshTokenInvalid)?;
        let connection = self
            .connections
            .in_transaction(tx)
            .find_by_id_for_update(connection_id)
            .await?
            .filter(|connection| connection.client_id() == client_id)
            .ok_or(OAuthServiceError::RefreshTokenInvalid)?;
        let (family, presented) = self
            .families
            .in_transaction(tx)
            .find_by_token_for_update(token)
            .await?
            .ok_or(OAuthServiceError::RefreshTokenInvalid)?;
        Ok((connection, family, presented))
    }

    async fn rotate(
        &self,
        tx: &mut U::Tx,
        connection: &mut OAuthConnection,
        family: &mut RefreshTokenFamily,
        scopes: HashSet<Scope>,
        now: OffsetDateTime,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let rotation = family.rotate(self.metadata_document_clients.lifetimes(), now);
        self.families
            .in_transaction(tx)
            .record_rotation(family, &rotation.replaced)
            .await?;
        let issued = self
            .metadata_document_clients
            .issue_access_token(
                &mut self.access_tokens.in_transaction(tx),
                connection,
                scopes,
                now,
            )
            .await?;
        self.connections
            .in_transaction(tx)
            .update(connection)
            .await?;
        Ok(issued.into_response(Some(rotation.raw_token)))
    }

    /// Someone else may hold the family: end it and every access token of the connection.
    async fn revoke_for_reuse(
        &self,
        tx: &mut U::Tx,
        connection: &OAuthConnection,
        family: &RefreshTokenFamily,
    ) -> Result<(), OAuthServiceError> {
        self.families.in_transaction(tx).delete(family.id()).await?;
        let revoked_access_tokens = self
            .access_tokens
            .in_transaction(tx)
            .delete_by_oauth_holder(
                connection.user_id(),
                OAuthTokenHolder::Connection(connection.id()),
            )
            .await?;
        tracing::warn!(
            event = "oauth_refresh_token.reused",
            user_id = %connection.user_id(),
            connection_id = %connection.id(),
            client_host = connection.client_id().host(),
            revoked_access_tokens,
            outcome = "failure",
        );
        Ok(())
    }
}
