use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, OAuthClientRepositoryFactory,
    OAuthConnectionRepository, OAuthConnectionRepositoryFactory, RefreshTokenFamilyRepository,
    RefreshTokenFamilyRepositoryFactory, ThirdPartyExchangeCodeRepository,
    ThirdPartyExchangeCodeRepositoryFactory,
};
use crate::use_cases::metadata_document_client_credentials::MetadataDocumentClientCredentials;
use crate::use_cases::support::{THIRD_PARTY_EXCHANGE_CODE_TTL, authenticate_client, verify_s256};
use application::error::box_error;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{
    AuthorizationCode, OAuthAuthorizationCode, OAuthCodeVerifier,
};
use oauth_core::client::OAuthClient;
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_core::refresh_token::{RawRefreshToken, RefreshTokenFamily};
use oauth_core::resource::{AuraResource, AuraResourcePolicy};
use oauth_core::third_party_exchange_code::{
    RehydratedThirdPartyExchangeCodeGrantState, ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    RawOAuthClientSecret, Scope,
};
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthGrantType {
    AuthorizationCode,
    RefreshToken,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthTokenType {
    Bearer,
}

/// How the client identifies itself at the token endpoint. Each client kind has exactly one
/// method: registered clients use `client_secret_post`, metadata document clients `none`.
#[derive(Debug, Clone, PartialEq)]
pub enum OAuthTokenClientAuthentication {
    ClientSecretPost {
        client_id: OAuthClientId,
        client_secret: RawOAuthClientSecret,
    },
    None {
        client_id: MetadataDocumentClientId,
    },
}

impl OAuthTokenClientAuthentication {
    pub fn client_id(&self) -> OAuthClientIdentifier {
        match self {
            Self::ClientSecretPost { client_id, .. } => (*client_id).into(),
            Self::None { client_id } => client_id.clone().into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenByAuthorizationCodeRequest {
    pub grant_type: OAuthGrantType,
    pub code: OAuthAuthorizationCode,
    pub redirect_uri: url::Url,
    pub resource: Option<String>,
    pub client: OAuthTokenClientAuthentication,
    pub code_verifier: OAuthCodeVerifier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenResponse {
    pub access_token: RawAccessToken,
    pub token_type: OAuthTokenType,
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub third_party_exchange_code: Option<ThirdPartyExchangeCode>,
    /// Issued only to metadata document clients; each refresh replaces it.
    pub refresh_token: Option<RawRefreshToken>,
}

#[async_trait::async_trait]
pub trait TokenByAuthorizationCodeUseCase: Send + Sync {
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

/// The persistence a token exchange writes to.
pub struct TokenExchangeStores<C, A, T, R, N, F> {
    pub clients: C,
    pub codes: A,
    pub exchange_codes: T,
    pub access_tokens: R,
    pub connections: N,
    pub refresh_token_families: F,
}

pub struct TokenByAuthorizationCodeHandler<U, C, A, T, R, N, F> {
    unit_of_work: U,
    stores: TokenExchangeStores<C, A, T, R, N, F>,
    metadata_document_clients: MetadataDocumentClientCredentials,
    resource_policy: AuraResourcePolicy,
}
impl<U, C, A, T, R, N, F> TokenByAuthorizationCodeHandler<U, C, A, T, R, N, F> {
    pub fn new(
        unit_of_work: U,
        stores: TokenExchangeStores<C, A, T, R, N, F>,
        metadata_document_clients: MetadataDocumentClientCredentials,
        resource_policy: AuraResourcePolicy,
    ) -> Self {
        Self {
            unit_of_work,
            stores,
            metadata_document_clients,
            resource_policy,
        }
    }
}

/// The client the token endpoint authenticated.
enum TokenClient {
    Registered(Box<OAuthClient>),
    /// Admitted by policy; its document was validated when the code was approved.
    MetadataDocument(MetadataDocumentClientId),
}

#[async_trait::async_trait]
impl<U, C, A, T, R, N, F> TokenByAuthorizationCodeUseCase
    for TokenByAuthorizationCodeHandler<U, C, A, T, R, N, F>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
    T: ThirdPartyExchangeCodeRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let requested_resource = request
            .resource
            .as_deref()
            .map(|resource| self.resource_policy.resolve(Some(resource)))
            .transpose()
            .map_err(|_| OAuthServiceError::InvalidResource)?;
        if let OAuthTokenClientAuthentication::None { client_id } = &request.client {
            self.metadata_document_clients.admit(client_id)?;
        }
        let mut tx = self.unit_of_work.begin().await?;
        let client = match &request.client {
            OAuthTokenClientAuthentication::ClientSecretPost {
                client_id,
                client_secret,
            } => TokenClient::Registered(Box::new(
                authenticate_client(
                    &mut self.stores.clients.in_transaction(&mut tx),
                    client_id,
                    client_secret,
                )
                .await?,
            )),
            OAuthTokenClientAuthentication::None { client_id } => {
                TokenClient::MetadataDocument(client_id.clone())
            }
        };
        let code = self
            .stores
            .codes
            .in_transaction(&mut tx)
            .consume_by_code(&request.code)
            .await?
            .ok_or(OAuthServiceError::AuthorizationCodeNotFound)?;
        let granted_resource = match self.verify_exchange(&request, requested_resource, &code) {
            Ok(resource) => resource,
            // Invalid persisted state rolls back. So does a public client presenting another
            // client's code: it proved no credential, so it must not burn that code.
            Err(error @ OAuthServiceError::InvalidPersistedState { .. }) => return Err(error),
            Err(error @ OAuthServiceError::AuthorizationCodeClientMismatch)
                if matches!(request.client, OAuthTokenClientAuthentication::None { .. }) =>
            {
                return Err(error);
            }
            // Any other rejected exchange still consumes the code.
            Err(error) => {
                tx.commit().await?;
                return Err(error);
            }
        };
        let response = match &client {
            TokenClient::Registered(client) => {
                self.issue_registered_client_credentials(&mut tx, client, &code)
                    .await?
            }
            TokenClient::MetadataDocument(client_id) => {
                match self
                    .issue_metadata_document_client_credentials(
                        &mut tx,
                        client_id,
                        &code,
                        granted_resource,
                    )
                    .await
                {
                    Ok(response) => response,
                    // The user ended the connection after approving; the code is spent.
                    Err(error @ OAuthServiceError::ConnectionNotFound) => {
                        tx.commit().await?;
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        tx.commit().await?;
        Ok(response)
    }
}

impl<U, C, A, T, R, N, F> TokenByAuthorizationCodeHandler<U, C, A, T, R, N, F>
where
    U: UnitOfWork,
    T: ThirdPartyExchangeCodeRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
    N: OAuthConnectionRepositoryFactory<U::Tx>,
    F: RefreshTokenFamilyRepositoryFactory<U::Tx>,
{
    /// Checks that the exchange presents what the code was bound to at approval, and
    /// returns the resource the grant is bound to.
    fn verify_exchange(
        &self,
        request: &TokenByAuthorizationCodeRequest,
        requested_resource: Option<AuraResource>,
        code: &AuthorizationCode,
    ) -> Result<AuraResource, OAuthServiceError> {
        if code.is_expired_at(OffsetDateTime::now_utc()) {
            return Err(OAuthServiceError::AuthorizationCodeExpired);
        }
        if code.client_id() != &request.client.client_id() {
            return Err(OAuthServiceError::AuthorizationCodeClientMismatch);
        }
        if code.redirect_uri() != &request.redirect_uri {
            return Err(OAuthServiceError::AuthorizationCodeRedirectUriMismatch);
        }
        let granted_resource = self
            .resource_policy
            .resolve(code.resource().map(|resource| resource.as_str()))
            .map_err(|source| OAuthServiceError::InvalidPersistedState {
                source: box_error(source),
            })?;
        if requested_resource.is_some_and(|resource| resource != granted_resource) {
            return Err(OAuthServiceError::AuthorizationCodeResourceMismatch);
        }
        if !verify_s256(&request.code_verifier, code.code_challenge()) {
            return Err(OAuthServiceError::InvalidCodeVerifier);
        }
        Ok(granted_resource)
    }

    /// A confidential client receives the first-party default of a non-expiring token and a
    /// one-time third-party exchange code.
    async fn issue_registered_client_credentials(
        &self,
        tx: &mut U::Tx,
        client: &OAuthClient,
        code: &AuthorizationCode,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let raw_access_token = RawAccessToken::new();
        let issued_at = OffsetDateTime::now_utc();
        let access_token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw_access_token.clone().into(),
            user_id: code.user_id(),
            name: AccessTokenName::from(
                format!("{} (OAuth-Client {})", client.name(), client.client_id()).as_str(),
            ),
            scopes: code.scopes().clone(),
            origin: AccessTokenOrigin::OAuth {
                client_id: client.client_id(),
            },
            expires: None,
        });
        self.stores
            .access_tokens
            .in_transaction(tx)
            .insert(&access_token)
            .await?;
        let third_party_exchange_code =
            ThirdPartyExchangeCodeGrant::create(RehydratedThirdPartyExchangeCodeGrantState {
                code: ThirdPartyExchangeCode::new(),
                access_token_id: access_token.id(),
                access_token: raw_access_token.clone(),
                access_token_expires: access_token.expires(),
                scopes: access_token.scopes().clone(),
                expires: issued_at + THIRD_PARTY_EXCHANGE_CODE_TTL,
            });
        self.stores
            .exchange_codes
            .in_transaction(tx)
            .insert(third_party_exchange_code.clone())
            .await?;
        Ok(TokenResponse {
            access_token: raw_access_token,
            token_type: OAuthTokenType::Bearer,
            expires: access_token.expires(),
            scopes: access_token.scopes().clone(),
            third_party_exchange_code: Some(third_party_exchange_code.code()),
            refresh_token: None,
        })
    }

    /// A public client never receives a non-expiring token or a third-party exchange code. Its
    /// token belongs to the connection the approval recorded, and a refresh-token family starts
    /// when the code allows one.
    async fn issue_metadata_document_client_credentials(
        &self,
        tx: &mut U::Tx,
        client_id: &MetadataDocumentClientId,
        code: &AuthorizationCode,
        resource: AuraResource,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let now = OffsetDateTime::now_utc();
        let mut connection = self
            .stores
            .connections
            .in_transaction(tx)
            .find_by_user_and_client_for_update(code.user_id(), client_id)
            .await?
            .ok_or(OAuthServiceError::ConnectionNotFound)?;
        let refresh_token = if code.issues_refresh_token() {
            let (family, raw_token) = RefreshTokenFamily::start(
                connection.id(),
                code.scopes().clone(),
                resource,
                self.metadata_document_clients.lifetimes(),
                now,
            );
            self.stores
                .refresh_token_families
                .in_transaction(tx)
                .insert(&family)
                .await?;
            Some(raw_token)
        } else {
            None
        };
        let issued = self
            .metadata_document_clients
            .issue_access_token(
                &mut self.stores.access_tokens.in_transaction(tx),
                &mut connection,
                code.scopes().clone(),
                now,
            )
            .await?;
        self.stores
            .connections
            .in_transaction(tx)
            .update(&connection)
            .await?;
        Ok(issued.into_response(refresh_token))
    }
}
