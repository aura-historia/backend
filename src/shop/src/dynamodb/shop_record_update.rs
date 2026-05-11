use crate::dynamodb::shop_type_record::ShopTypeRecord;
use common::{domain::Domain, dynamodb_update::DynamoDbUpdate, user_id::UserId};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
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
    pub gsi3_pk: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi3_sk: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_domain: Option<Domain>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_country: Option<CountryCode>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address_lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address_lon: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_api_key_short: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_api_key_long_hash: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for ShopRecordUpdate {}
