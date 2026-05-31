use crate::core::client::{OAuthClient, OAuthClientName};
use common::actor::record::ActorRecord;
use common::oauth_client_id::OAuthClientId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;
use user::core::access_token::HashedRawOAuthClientSecret;
use user::dynamodb::access_token_record::ScopeRecord;

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
    pub created_by: ActorRecord,
    pub updated_by: ActorRecord,
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

impl From<OAuthClient> for OAuthClientRecord {
    fn from(client: OAuthClient) -> Self {
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
            created_by: client.created_by.into(),
            updated_by: client.updated_by.into(),
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
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
        }
    }
}
