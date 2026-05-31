use crate::dynamodb::access_token_record::ScopeRecord;
use common::{actor::record::ActorRecord, dynamodb_update::DynamoDbUpdate};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct AccessTokenRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<HashSet<ScopeRecord>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for AccessTokenRecordUpdate {}
