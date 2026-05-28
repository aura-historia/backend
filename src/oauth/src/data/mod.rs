use crate::service::oauth_service::{IntrospectionResponse, OAuthTokenType, TokenResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use user::data::access_token_data::{AccessTokenTypeData, ScopeData};

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
