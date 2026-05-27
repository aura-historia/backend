use crate::core::access_token::{AccessToken, AccessTokenId, RawAccessToken, Scope};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeData {
    ShopsManage,
    ProductsWrite,
}

impl From<Scope> for ScopeData {
    fn from(value: Scope) -> Self {
        match value {
            Scope::ShopsManage => ScopeData::ShopsManage,
            Scope::ProductsWrite => ScopeData::ProductsWrite,
        }
    }
}

impl From<ScopeData> for Scope {
    fn from(value: ScopeData) -> Self {
        match value {
            ScopeData::ShopsManage => Scope::ShopsManage,
            ScopeData::ProductsWrite => Scope::ProductsWrite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AccessTokenTypeData {
    Bearer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawAccessTokenData(pub String);

impl From<RawAccessToken> for RawAccessTokenData {
    fn from(value: RawAccessToken) -> Self {
        RawAccessTokenData(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccessTokenData {
    pub access_token_id: AccessTokenId,
    pub name: String,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub scope: HashSet<ScopeData>,
    pub token_type: AccessTokenTypeData,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedAccessTokenData {
    #[serde(flatten)]
    pub metadata: GetAccessTokenData,
    pub access_token: RawAccessTokenData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostAccessTokenData {
    pub name: String,
    #[serde(default)]
    pub scope: HashSet<ScopeData>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchAccessTokenData {
    pub access_token_id: AccessTokenId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HashSet<ScopeData>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
}

impl From<AccessToken> for GetAccessTokenData {
    fn from(access_token: AccessToken) -> Self {
        let now = OffsetDateTime::now_utc();
        GetAccessTokenData {
            access_token_id: access_token.id,
            name: access_token.name.into(),
            scope: access_token.scopes.into_iter().map(Into::into).collect(),
            token_type: AccessTokenTypeData::Bearer,
            expires_at: access_token.expires,
            expires_in: access_token
                .expires
                .map(|expires| (expires - now).whole_seconds().max(0)),
            created: access_token.created,
            updated: access_token.updated,
        }
    }
}

impl From<(RawAccessToken, AccessToken)> for CreatedAccessTokenData {
    fn from((raw, access_token): (RawAccessToken, AccessToken)) -> Self {
        CreatedAccessTokenData {
            metadata: access_token.into(),
            access_token: raw.into(),
        }
    }
}
