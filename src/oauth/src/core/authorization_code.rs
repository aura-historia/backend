use crate::core::client::OAuthClientId;
use common::{user_id::UserId, uuid_v7_newtype};
use std::collections::HashSet;
use time::OffsetDateTime;
use user::core::access_token::Scope;

uuid_v7_newtype!(OAuthAuthorizationCode);

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizationCode {
    pub code: OAuthAuthorizationCode,
    pub client_id: OAuthClientId,
    pub user_id: UserId,
    pub redirect_uri: String,
    pub scopes: HashSet<Scope>,
    pub code_challenge: String,
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
