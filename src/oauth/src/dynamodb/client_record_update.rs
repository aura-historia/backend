use common::{actor::record::ActorRecord, dynamodb_update::DynamoDbUpdate};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user::dynamodb::access_token_record::ScopeRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct OAuthClientRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<crate::core::client::OAuthClientName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uris: Option<HashSet<crate::core::client::OAuthRedirectUri>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<HashSet<ScopeRecord>>,

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for OAuthClientRecordUpdate {}
