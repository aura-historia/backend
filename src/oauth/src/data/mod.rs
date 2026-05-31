use crate::core::client::{OAuthClient, OAuthClientName};
use crate::service::oauth_service::{IntrospectionResponse, OAuthTokenType, TokenResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user::core::access_token::RawOAuthClientSecret;
use user::data::access_token_data::{AccessTokenTypeData, ScopeData};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthClientMetadataRequestData {
    pub client_name: String,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    #[serde(default)]
    pub redirect_uris: HashSet<url::Url>,
    #[serde(default)]
    pub scope: HashSet<ScopeData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthClientMetadataPatchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uris: Option<HashSet<url::Url>>,
    pub tos_uri: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_uri: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HashSet<ScopeData>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OAuthClientMetadataResponseData {
    pub client_id: String,
    pub client_secret: String,
    pub client_name: String,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub redirect_uris: HashSet<String>,
    pub scope: HashSet<ScopeData>,
    pub client_id_issued_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenResponseData {
    pub access_token: String,
    pub token_type: AccessTokenTypeData,
    pub expires_in: Option<i64>,
    pub scope: String,
}

impl From<TokenResponse> for TokenResponseData {
    fn from(response: TokenResponse) -> Self {
        Self {
            access_token: response.access_token.into(),
            token_type: response.token_type.into(),
            expires_in: response
                .expires
                .map(|expires| (expires - OffsetDateTime::now_utc()).whole_seconds().max(0)),
            scope: scope_string(&response.scopes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntrospectionResponseData {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<i64>,
}

impl From<OAuthClient> for OAuthClientMetadataResponseData {
    fn from(client: OAuthClient) -> Self {
        Self {
            client_id: client.client_id.into(),
            client_secret: client.hashed_client_secret.to_string(),
            client_name: client.name.into(),
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            redirect_uris: client.redirect_uris.into_iter().map(Into::into).collect(),
            scope: client.scopes.into_iter().map(Into::into).collect(),
            client_id_issued_at: client.created.unix_timestamp(),
        }
    }
}

impl From<(RawOAuthClientSecret, OAuthClient)> for OAuthClientMetadataResponseData {
    fn from((secret, client): (RawOAuthClientSecret, OAuthClient)) -> Self {
        let mut response = OAuthClientMetadataResponseData::from(client);
        response.client_secret = secret.into();
        response
    }
}

impl From<OAuthClientMetadataRequestData>
    for crate::service::oauth_service::CreateOAuthClientCommand
{
    fn from(data: OAuthClientMetadataRequestData) -> Self {
        Self {
            name: OAuthClientName::from(data.client_name),
            redirect_uris: data.redirect_uris,
            tos_uri: data.tos_uri,
            policy_uri: data.policy_uri,
            client_uri: data.client_uri,
            logo_uri: data.logo_uri,
            scopes: data.scope.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<OAuthClientMetadataPatchData>
    for crate::service::oauth_service::UpdateOAuthClientCommand
{
    fn from(data: OAuthClientMetadataPatchData) -> Self {
        Self {
            name: data.client_name.map(OAuthClientName::from),
            redirect_uris: data.redirect_uris,
            tos_uri: data.tos_uri,
            policy_uri: data.policy_uri,
            client_uri: data.client_uri,
            logo_uri: data.logo_uri,
            scopes: data
                .scope
                .map(|scopes| scopes.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<IntrospectionResponse> for IntrospectionResponseData {
    fn from(response: IntrospectionResponse) -> Self {
        Self {
            active: response.active,
            scope: response.scopes.as_ref().map(scope_string),
            client_id: response.client_id.map(Into::into),
            sub: response.subject.map(|user_id| user_id.to_string()),
            token_type: response.token_type.map(String::from),
            exp: response.expires.map(|expires| expires.unix_timestamp()),
            iat: response
                .issued_at
                .map(|issued_at| issued_at.unix_timestamp()),
        }
    }
}

impl From<OAuthTokenType> for AccessTokenTypeData {
    fn from(value: OAuthTokenType) -> Self {
        match value {
            OAuthTokenType::Bearer => AccessTokenTypeData::Bearer,
        }
    }
}

impl From<OAuthTokenType> for String {
    fn from(value: OAuthTokenType) -> Self {
        match value {
            OAuthTokenType::Bearer => "Bearer".to_owned(),
        }
    }
}

/// Converts scopes into the OAuth space-separated scope string used in REST responses.
pub fn scope_string(scopes: &HashSet<user::core::access_token::Scope>) -> String {
    let mut values = scopes
        .iter()
        .copied()
        .map(ScopeData::from)
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values.join(" ")
}
