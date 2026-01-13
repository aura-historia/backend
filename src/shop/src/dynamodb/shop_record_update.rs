use crate::dynamodb::shop_type_record::ShopTypeRecord;
use common::{domain::Domain, dynamodb_update::DynamoDbUpdate, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopRecordUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<ShopName>,

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
