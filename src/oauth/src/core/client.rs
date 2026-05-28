use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user::core::access_token::{HashedRawAccessToken, Scope};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OAuthClientId(pub String);

impl std::fmt::Display for OAuthClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for OAuthClientId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for OAuthClientId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<OAuthClientId> for String {
    fn from(value: OAuthClientId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClient {
    pub client_id: OAuthClientId,
    pub hashed_client_secret: HashedRawAccessToken,
    pub name: String,
    pub redirect_uris: HashSet<String>,
    pub scopes: HashSet<Scope>,
    pub created_by: UserId,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
