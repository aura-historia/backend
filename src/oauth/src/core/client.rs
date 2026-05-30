use common::{actor::domain::Actor, oauth_client_id::OAuthClientId, string_newtype};
use std::collections::HashSet;
use time::OffsetDateTime;
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
    pub redirect_uris: HashSet<OAuthRedirectUri>,
    pub scopes: HashSet<Scope>,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
