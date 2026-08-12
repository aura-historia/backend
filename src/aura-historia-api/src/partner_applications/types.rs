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
use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
use shop_partner_service::ports::PartnerShopApplicationView;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OwnPartnerApplicationData {
    pub(crate) id: PartnerShopApplicationId,
    pub(crate) business_state: PartnerShopApplicationStateData,
    pub(crate) payload: PartnerApplicationPayloadData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminPartnerApplicationData {
    pub(crate) id: PartnerShopApplicationId,
    pub(crate) applicant_user_id: UserId,
    pub(crate) business_state: PartnerShopApplicationStateData,
    pub(crate) payload: PartnerApplicationPayloadData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PartnerShopApplicationStateData {
    Submitted,
    InReview,
    Rejected,
    Approved,
    Withdrawn,
}

impl From<PartnerShopApplicationState> for PartnerShopApplicationStateData {
    fn from(state: PartnerShopApplicationState) -> Self {
        match state {
            PartnerShopApplicationState::Submitted => Self::Submitted,
            PartnerShopApplicationState::InReview => Self::InReview,
            PartnerShopApplicationState::Rejected => Self::Rejected,
            PartnerShopApplicationState::Approved => Self::Approved,
            PartnerShopApplicationState::Withdrawn => Self::Withdrawn,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub(crate) enum PartnerApplicationPayloadData {
    Existing { shop_id: ShopId },
    New { shop_id: ShopId },
}

fn payload_data(a: &PartnerShopApplication) -> PartnerApplicationPayloadData {
    match a.payload() {
        PartnerShopApplicationPayload::Existing { shop_id } => {
            PartnerApplicationPayloadData::Existing { shop_id }
        }
        PartnerShopApplicationPayload::New { shop_id } => {
            PartnerApplicationPayloadData::New { shop_id }
        }
    }
}

impl From<PartnerShopApplication> for OwnPartnerApplicationData {
    fn from(a: PartnerShopApplication) -> Self {
        Self {
            id: a.id(),
            business_state: a.business_state().into(),
            payload: payload_data(&a),
        }
    }
}

impl From<PartnerShopApplicationView> for OwnPartnerApplicationData {
    fn from(v: PartnerShopApplicationView) -> Self {
        Self {
            id: v.id,
            business_state: v.business_state.into(),
            payload: match v.payload {
                PartnerShopApplicationPayload::Existing { shop_id } => {
                    PartnerApplicationPayloadData::Existing { shop_id }
                }
                PartnerShopApplicationPayload::New { shop_id } => {
                    PartnerApplicationPayloadData::New { shop_id }
                }
            },
        }
    }
}

impl From<PartnerShopApplication> for AdminPartnerApplicationData {
    fn from(a: PartnerShopApplication) -> Self {
        Self {
            id: a.id(),
            applicant_user_id: a.applicant_user_id(),
            business_state: a.business_state().into(),
            payload: payload_data(&a),
        }
    }
}

impl From<PartnerShopApplicationView> for AdminPartnerApplicationData {
    fn from(v: PartnerShopApplicationView) -> Self {
        Self {
            id: v.id,
            applicant_user_id: v.applicant_user_id,
            business_state: v.business_state.into(),
            payload: match v.payload {
                PartnerShopApplicationPayload::Existing { shop_id } => {
                    PartnerApplicationPayloadData::Existing { shop_id }
                }
                PartnerShopApplicationPayload::New { shop_id } => {
                    PartnerApplicationPayloadData::New { shop_id }
                }
            },
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
pub(crate) struct DecisionData {
    pub(crate) decision: PartnerApplicationDecisionData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum PartnerApplicationDecisionData {
    Approve,
    Reject,
}
