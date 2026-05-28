use crate::core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
};
use crate::core::client::{OAuthClientId, OAuthRedirectUri};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user::dynamodb::access_token_record::ScopeRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct AuthorizationCodeRecord {
    pub pk: String,
    pub sk: String,
    pub code: OAuthAuthorizationCode,
    pub client_id: OAuthClientId,
    pub user_id: common::user_id::UserId,
    pub redirect_uri: OAuthRedirectUri,
    pub scopes: HashSet<ScopeRecord>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
    pub expires: i64,
    pub ttl: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

pub fn mk_pk(code: &OAuthAuthorizationCode) -> String {
    format!("oauth_authorization_code#{code}")
}

pub fn mk_sk() -> &'static str {
    "oauth_authorization_code"
}

impl From<AuthorizationCode> for AuthorizationCodeRecord {
    fn from(code: AuthorizationCode) -> Self {
        Self {
            pk: mk_pk(&code.code),
            sk: mk_sk().to_owned(),
            code: code.code,
            client_id: code.client_id,
            user_id: code.user_id,
            redirect_uri: code.redirect_uri,
            scopes: code.scopes.into_iter().map(Into::into).collect(),
            code_challenge: code.code_challenge,
            code_challenge_method: code.code_challenge_method,
            expires: code.expires.unix_timestamp(),
            ttl: code.expires.unix_timestamp(),
            created: code.created,
        }
    }
}

impl From<AuthorizationCodeRecord> for AuthorizationCode {
    fn from(record: AuthorizationCodeRecord) -> Self {
        Self {
            code: record.code,
            client_id: record.client_id,
            user_id: record.user_id,
            redirect_uri: record.redirect_uri,
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            code_challenge: record.code_challenge,
            code_challenge_method: record.code_challenge_method,
            expires: OffsetDateTime::from_unix_timestamp(record.expires)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            created: record.created,
        }
    }
}
