use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClientName;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::Scope;

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientView {
    pub client_id: OAuthClientId,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
