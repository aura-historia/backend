use crate::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use crate::dynamodb::partner_status_record::ShopPartnerStatusRecord;
use crate::dynamodb::shop_type_record::ShopTypeRecord;
use common::actor::record::ActorRecord;
use common::currency::record::CurrencyRecord;
use common::language::record::LanguageRecord;
use common::{domain::Domain, dynamodb_update::DynamoDbUpdate};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopRecordUpdate {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi3_pk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gsi3_sk: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_partner_status: Option<ShopPartnerStatusRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_domain: Option<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_currency: Option<CurrencyRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_language: Option<LanguageRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_currency: Option<CurrencyRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub woocommerce_language: Option<LanguageRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<Url>,

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

    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for ShopRecordUpdate {}

impl Default for ShopRecordUpdate {
    fn default() -> Self {
        Self {
            gsi3_pk: None,
            gsi3_sk: None,
            shop_type: None,
            domains: None,
            image: None,
            shop_partner_status: None,
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: None,
            view_url: None,
            structured_address_addressline: None,
            structured_address_addressline_extra: None,
            structured_address_locality: None,
            structured_address_region: None,
            structured_address_postal_code: None,
            structured_address_country: None,
            geo_address_lat: None,
            geo_address_lon: None,
            phone: None,
            email: None,
            updated_by: ActorRecord::System,
            updated: OffsetDateTime::now_utc(),
        }
    }
}
