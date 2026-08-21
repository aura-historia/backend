use credential_core::{oauth_client_id::OAuthClientId, scope::Scope};
use domain_primitives::string_newtype;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::HashedRawOAuthClientSecret;

string_newtype!(
    OAuthClientName,
    derives(serde::Serialize, serde::Deserialize)
);

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClient {
    pub client_id: OAuthClientId,
    pub hashed_client_secret: HashedRawOAuthClientSecret,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<url::Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
