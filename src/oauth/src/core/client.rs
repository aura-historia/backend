use common::{oauth_client_id::OAuthClientId, string_newtype, user_id::UserId};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user::core::access_token::{HashedRawOAuthClientSecret, Scope};

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
    pub hashed_client_secret: HashedRawOAuthClientSecret,
    pub name: OAuthClientName,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub redirect_uris: HashSet<OAuthRedirectUri>,
    pub scopes: HashSet<Scope>,
    pub created_by: UserId,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
