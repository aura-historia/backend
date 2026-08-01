use crate::shops::types::ShopTypeData;
use common::currency::data::CurrencyData;
use common::domain::Domain;
use common::language::data::LanguageData;
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::user_id::UserId;
use geo::data::address_data::StructuredAddressData;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationPayload,
};
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartnerApplicationData {
    pub(crate) id: PartnerShopApplicationId,
    pub(crate) applicant_user_id: UserId,
    pub(crate) business_state: String,
    pub(crate) execution_state: String,
    pub(crate) payload: PartnerApplicationPayloadData,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum PartnerApplicationPayloadData {
    Existing { shop_id: ShopId },
    New { shop_id: ShopId },
}
impl From<PartnerShopApplication> for PartnerApplicationData {
    fn from(a: PartnerShopApplication) -> Self {
        let payload = match a.payload() {
            PartnerShopApplicationPayload::Existing { shop_id } => {
                PartnerApplicationPayloadData::Existing { shop_id }
            }
            PartnerShopApplicationPayload::New { shop_id } => {
                PartnerApplicationPayloadData::New { shop_id }
            }
        };
        Self {
            id: a.id(),
            applicant_user_id: a.applicant_user_id(),
            business_state: format!("{:?}", a.business_state()),
            execution_state: format!("{:?}", a.execution_state()),
            payload,
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub(crate) enum PostPayloadData {
    #[serde(rename = "EXISTING", alias = "Existing", alias = "existing")]
    Existing {
        #[serde(alias = "shop_id")]
        shop_id: ShopId,
    },
    #[serde(rename = "NEW", alias = "New", alias = "new")]
    New {
        shop_name: ShopName,
        shop_type: ShopTypeData,
        shop_domains: HashSet<Domain>,
        #[serde(default)]
        shopify_domain: Option<Domain>,
        #[serde(default)]
        shopify_currency: Option<CurrencyData>,
        #[serde(default)]
        shopify_language: Option<LanguageData>,
        #[serde(default)]
        woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
        #[serde(default)]
        woocommerce_currency: Option<CurrencyData>,
        #[serde(default)]
        woocommerce_language: Option<LanguageData>,
        #[serde(default)]
        shop_url: Option<Url>,
        #[serde(default)]
        shop_image: Option<Url>,
        #[serde(default)]
        shop_structured_address: Option<StructuredAddressData>,
        #[serde(default)]
        shop_phone: Option<String>,
        #[serde(default)]
        shop_email: Option<Email>,
    },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostApplicationData {
    pub(crate) payload: PostPayloadData,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchApplicationData {
    pub(crate) task_token: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecisionData {
    pub(crate) decision: String,
}
