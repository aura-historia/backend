use super::PersistedOAuthClient;
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

impl From<PersistedOAuthClient> for OAuthClientView {
    fn from(persisted: PersistedOAuthClient) -> Self {
        let client = persisted.value;
        Self {
            client_id: client.client_id(),
            name: client.name().clone(),
            redirect_uris: client.redirect_uris().as_set().clone(),
            tos_uri: client.tos_uri().clone(),
            policy_uri: client.policy_uri().clone(),
            client_uri: client.client_uri().clone(),
            logo_uri: client.logo_uri().clone(),
            scopes: client.scopes().clone(),
            created: persisted.created,
            updated: persisted.updated,
        }
    }
}
