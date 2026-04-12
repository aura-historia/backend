use crate::dynamodb::shop_type_record::ShopTypeRecord;
use common::{domain::Domain, dynamodb_update::DynamoDbUpdate, user_id::UserId};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopRecordUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_user_id: Option<UserId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi1_pk: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi1_sk: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for ShopRecordUpdate {}
