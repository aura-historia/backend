use common::{string_newtype, user_id::UserId};
use std::collections::HashSet;
use time::OffsetDateTime;
use user::core::access_token::{HashedRawAccessToken, Scope};

string_newtype!(OAuthClientId, derives(serde::Serialize, serde::Deserialize));
string_newtype!(
    OAuthClientName,
    derives(serde::Serialize, serde::Deserialize)
);
string_newtype!(
    OAuthRedirectUri,
    derives(serde::Serialize, serde::Deserialize)
);

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClient {
    pub client_id: OAuthClientId,
    pub hashed_client_secret: HashedRawAccessToken,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<OAuthRedirectUri>,
    pub scopes: HashSet<Scope>,
    pub created_by: UserId,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
