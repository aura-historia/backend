#![allow(dead_code)]

use oauth_core::client::OAuthClient;
use oauth_service::use_cases::{
    CreateOAuthClientResult, IntrospectTokenResponse, OAuthTokenType, TokenResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::Scope;

fn scope_strings(scopes: HashSet<Scope>) -> HashSet<String> {
    scopes
        .into_iter()
        .map(|scope| scope.as_str().to_owned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OAuthClientMetadataRequestDto {
    pub name: oauth_core::client::OAuthClientName,
    pub redirect_uris: HashSet<url::Url>,
    pub tos_uri: url::Url,
    pub policy_uri: url::Url,
    pub client_uri: url::Url,
    pub logo_uri: url::Url,
    pub scopes: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OAuthClientMetadataResponseDto {
    pub client_id: common::oauth_client_id::OAuthClientId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub name: oauth_core::client::OAuthClientName,
    pub redirect_uris: HashSet<url::Url>,
    pub tos_uri: url::Url,
    pub policy_uri: url::Url,
    pub client_uri: url::Url,
    pub logo_uri: url::Url,
    pub scopes: HashSet<String>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TokenResponseDto {
    pub access_token: String,
    pub token_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub third_party_exchange_code:
        Option<oauth_core::third_party_exchange_code::ThirdPartyExchangeCode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct IntrospectionResponseDto {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<common::oauth_client_id::OAuthClientId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<common::user_id::UserId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<i64>,
}

impl From<OAuthClient> for OAuthClientMetadataResponseDto {
    fn from(client: OAuthClient) -> Self {
        Self {
            client_id: client.client_id,
            client_secret: None,
            name: client.name,
            redirect_uris: client.redirect_uris,
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            scopes: scope_strings(client.scopes),
            created: client.created,
            updated: client.updated,
        }
    }
}

impl From<CreateOAuthClientResult> for OAuthClientMetadataResponseDto {
    fn from(result: CreateOAuthClientResult) -> Self {
        let mut dto = Self::from(result.client);
        dto.client_secret = Some(result.raw_client_secret.into());
        dto
    }
}

impl From<TokenResponse> for TokenResponseDto {
    fn from(result: TokenResponse) -> Self {
        let token_type = match result.token_type {
            OAuthTokenType::Bearer => "Bearer",
        };
        Self {
            access_token: result.access_token.into(),
            token_type,
            expires: result.expires,
            scopes: scope_strings(result.scopes),
            third_party_exchange_code: result.third_party_exchange_code,
        }
    }
}

impl From<IntrospectTokenResponse> for IntrospectionResponseDto {
    fn from(result: IntrospectTokenResponse) -> Self {
        let token_type = result.token_type.map(|token_type| match token_type {
            OAuthTokenType::Bearer => "Bearer",
        });
        let scope = result.scopes.map(|scopes| {
            scopes
                .into_iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });
        Self {
            active: result.active,
            scope,
            client_id: result.client_id,
            sub: result.subject,
            token_type,
            exp: result.expires.map(|expires| expires.unix_timestamp()),
            iat: result.issued_at.map(|issued_at| issued_at.unix_timestamp()),
        }
    }
}
