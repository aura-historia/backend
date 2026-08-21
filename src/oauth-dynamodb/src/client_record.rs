use crate::scope_record::ScopeRecord;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::{OAuthClient, OAuthClientName};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::{HashedRawOAuthClientSecret, RawOAuthClientSecret};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct OAuthClientRecord {
    pub pk: String,
    pub sk: String,
    pub client_id: OAuthClientId,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<url::Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<ScopeRecord>,
    pub secret_prefix: String,
    pub secret_short: String,
    pub secret_hash: String,
    pub secret: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk() -> &'static str {
    "oauth_clients"
}

pub fn mk_sk(client_id: &OAuthClientId) -> String {
    format!("oauth_client#{client_id}")
}

impl From<(OAuthClient, RawOAuthClientSecret)> for OAuthClientRecord {
    fn from((client, raw_secret): (OAuthClient, RawOAuthClientSecret)) -> Self {
        Self {
            pk: mk_pk().to_owned(),
            sk: mk_sk(&client.client_id),
            client_id: client.client_id,
            name: client.name,
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            redirect_uris: client.redirect_uris,
            scopes: client.scopes.into_iter().map(Into::into).collect(),
            secret_prefix: client.hashed_client_secret.prefix().to_owned(),
            secret_short: client.hashed_client_secret.short_token().to_owned(),
            secret_hash: client.hashed_client_secret.long_token_hash().to_owned(),
            secret: raw_secret.into(),
            created: client.created,
            updated: client.updated,
        }
    }
}

impl From<OAuthClientRecord> for OAuthClient {
    fn from(record: OAuthClientRecord) -> Self {
        Self {
            client_id: record.client_id,
            hashed_client_secret: HashedRawOAuthClientSecret::new(
                record.secret_short,
                record.secret_hash,
            ),
            name: record.name,
            tos_uri: record.tos_uri,
            policy_uri: record.policy_uri,
            client_uri: record.client_uri,
            logo_uri: record.logo_uri,
            redirect_uris: record.redirect_uris,
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            created: record.created,
            updated: record.updated,
        }
    }
}
