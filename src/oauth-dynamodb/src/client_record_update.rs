use crate::dynamodb_update::DynamoDbUpdate;
use crate::scope_record::ScopeRecord;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct OAuthClientRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<oauth_core::client::OAuthClientName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uris: Option<HashSet<url::Url>>,
    pub tos_uri: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_uri: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<HashSet<ScopeRecord>>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for OAuthClientRecordUpdate {}
