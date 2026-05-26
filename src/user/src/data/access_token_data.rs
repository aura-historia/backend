use crate::core::access_token::{AccessToken, AccessTokenId, RawAccessToken, Scope};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccessTokenData {
    pub access_token_id: AccessTokenId,
    pub name: String,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub scope: HashSet<Scope>,
    pub token_type: String,
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
    pub access_token: RawAccessToken,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostAccessTokenData {
    pub name: String,
    #[serde(default)]
    pub scope: HashSet<Scope>,
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
    pub scope: Option<HashSet<Scope>>,
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
            scope: access_token.scopes,
            token_type: "Bearer".to_owned(),
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
            access_token: raw,
        }
    }
}
