use credential_core::oauth_client_id::OAuthClientId;
use credential_core::scope::Scope;
use domain_primitives::{string_newtype, uuid_v7_newtype};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::user_id::UserId;

uuid_v7_newtype!(OAuthAuthorizationCode);
string_newtype!(
    OAuthCodeChallenge,
    derives(serde::Serialize, serde::Deserialize)
);
string_newtype!(OAuthCodeVerifier);

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizationCode {
    pub code: OAuthAuthorizationCode,
    pub client_id: OAuthClientId,
    pub user_id: UserId,
    pub redirect_uri: url::Url,
    pub scopes: HashSet<Scope>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
    pub expires: OffsetDateTime,
    pub created: OffsetDateTime,
}

impl AuthorizationCode {
    pub fn is_expired(&self) -> bool {
        self.expires < OffsetDateTime::now_utc()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CodeChallengeMethod {
    S256,
}
