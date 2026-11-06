use crate::error::OAuthServiceError;
use crate::use_cases::token_by_authorization_code::{OAuthTokenType, TokenResponse};
use application::error::box_error;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_core::connection::OAuthConnection;
use oauth_core::metadata_document_client::MetadataDocumentClientPolicy;
use oauth_core::refresh_token::{MetadataDocumentClientTokenLifetimes, RawRefreshToken};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_service::ports::AccessTokenRepository;

/// How the token endpoint treats metadata document clients: whether the operator admits a
/// client, and how long its credentials live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataDocumentClientCredentials {
    policy: MetadataDocumentClientPolicy,
    lifetimes: MetadataDocumentClientTokenLifetimes,
}

/// An access token issued within a connection.
pub(crate) struct IssuedAccessToken {
    raw_token: RawAccessToken,
    expires: OffsetDateTime,
    scopes: HashSet<Scope>,
}

impl MetadataDocumentClientCredentials {
    pub fn new(
        policy: MetadataDocumentClientPolicy,
        lifetimes: MetadataDocumentClientTokenLifetimes,
    ) -> Self {
        Self { policy, lifetimes }
    }

    pub fn policy(&self) -> &MetadataDocumentClientPolicy {
        &self.policy
    }

    pub(crate) fn lifetimes(&self) -> &MetadataDocumentClientTokenLifetimes {
        &self.lifetimes
    }

    /// Checked at every token request, so a newly denied host stops at its next request.
    pub(crate) fn admit(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<(), OAuthServiceError> {
        self.policy.admit(client_id).map_err(|source| {
            OAuthServiceError::InvalidMetadataDocumentClient {
                source: box_error(source),
            }
        })
    }

    /// Issues a short-lived access token within `connection` and records the use.
    pub(crate) async fn issue_access_token<R: AccessTokenRepository>(
        &self,
        access_tokens: &mut R,
        connection: &mut OAuthConnection,
        scopes: HashSet<Scope>,
        now: OffsetDateTime,
    ) -> Result<IssuedAccessToken, OAuthServiceError> {
        let raw_token = RawAccessToken::new();
        let expires = now + self.lifetimes.access_token();
        let access_token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw_token.clone().into(),
            user_id: connection.user_id(),
            name: AccessTokenName::from(
                format!(
                    "{} (OAuth-Client {})",
                    connection.client_name(),
                    connection.client_id()
                )
                .as_str(),
            ),
            scopes: scopes.clone(),
            origin: AccessTokenOrigin::OAuthMetadataDocument {
                client_id: connection.client_id().clone(),
                connection_id: connection.id(),
            },
            expires: Some(expires),
        });
        access_tokens.insert(&access_token).await?;
        connection.record_use(now);
        Ok(IssuedAccessToken {
            raw_token,
            expires,
            scopes,
        })
    }
}

impl IssuedAccessToken {
    pub(crate) fn into_response(self, refresh_token: Option<RawRefreshToken>) -> TokenResponse {
        TokenResponse {
            access_token: self.raw_token,
            token_type: OAuthTokenType::Bearer,
            expires: Some(self.expires),
            scopes: self.scopes,
            third_party_exchange_code: None,
            refresh_token,
        }
    }
}
