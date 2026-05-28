use crate::core::client::{OAuthClient, OAuthClientId, OAuthClientName, OAuthRedirectUri};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user::core::access_token::HashedRawOAuthClientSecret;
use user::dynamodb::access_token_record::ScopeRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct OAuthClientRecord {
    pub pk: String,
    pub sk: String,
    pub client_id: OAuthClientId,
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<OAuthRedirectUri>,
    pub scopes: HashSet<ScopeRecord>,
    pub secret_prefix: String,
    pub secret_short: String,
    pub secret_hash: String,
    pub created_by: common::user_id::UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(client_id: &OAuthClientId) -> String {
    format!("oauth_client#{client_id}")
}

pub fn mk_sk() -> &'static str {
    "oauth_client"
}

impl From<OAuthClient> for OAuthClientRecord {
    fn from(client: OAuthClient) -> Self {
        Self {
            pk: mk_pk(&client.client_id),
            sk: mk_sk().to_owned(),
            client_id: client.client_id,
            name: client.name,
            redirect_uris: client.redirect_uris,
            scopes: client.scopes.into_iter().map(Into::into).collect(),
            secret_prefix: client.hashed_client_secret.prefix().to_owned(),
            secret_short: client.hashed_client_secret.short_token().to_owned(),
            secret_hash: client.hashed_client_secret.long_token_hash().to_owned(),
            created_by: client.created_by,
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
            redirect_uris: record.redirect_uris,
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            created_by: record.created_by,
            created: record.created,
            updated: record.updated,
        }
    }
}
