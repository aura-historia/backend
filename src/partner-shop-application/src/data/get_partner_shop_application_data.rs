use crate::{
    core::{
        partner_shop_application::{PartnerShopApplication, PartnerShopApplicationPayload},
        partner_shop_application_id::PartnerShopApplicationId,
    },
    data::partner_shop_application_state_data::PartnerShopApplicationStateData,
};
use common::execution_state::data::ExecutionStateData;
use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName, user_id::UserId};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use shop::data::address_data::StructuredAddressData;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPartnerShopApplicationData {
    pub id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
    pub business_state: PartnerShopApplicationStateData,
    pub execution_state: ExecutionStateData,
    pub payload: GetPartnerShopApplicationPayloadData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum GetPartnerShopApplicationPayloadData {
    #[serde(rename = "EXISTING")]
    Existing { shop_id: ShopId },
    #[serde(rename = "NEW")]
    New {
        shop_name: ShopName,
        shop_type: ShopTypeData,
        shop_domains: HashSet<Domain>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_url: Option<Url>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_image: Option<Url>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_structured_address: Option<StructuredAddressData>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_phone: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_email: Option<Email>,
    },
}

impl From<PartnerShopApplication> for GetPartnerShopApplicationData {
    fn from(application: PartnerShopApplication) -> Self {
        let payload = match application.payload {
            PartnerShopApplicationPayload::Existing(shop_id) => {
                GetPartnerShopApplicationPayloadData::Existing { shop_id }
            }
            PartnerShopApplicationPayload::New(cmd) => GetPartnerShopApplicationPayloadData::New {
                shop_name: cmd.name,
                shop_type: cmd.shop_type.into(),
                shop_domains: cmd.domains,
                shop_url: cmd.url,
                shop_image: cmd.image,
                shop_structured_address: cmd.structured_address.map(Into::into),
                shop_phone: cmd.phone,
                shop_email: cmd.email,
            },
        };

        GetPartnerShopApplicationData {
            id: application.id,
            applicant_user_id: application.applicant_user_id,
            business_state: application.business_state.into(),
            execution_state: application.execution_state.into(),
            payload,
            created: application.created,
            updated: application.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::partner_shop_application::PartnerShopApplication;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for GetPartnerShopApplicationData {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            config
                .fake_with_rng::<PartnerShopApplication, R>(rng)
                .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GetPartnerShopApplicationData, GetPartnerShopApplicationPayloadData};
    use serde_json::json;

    #[test]
    fn should_roundtrip_existing_payload_when_using_camel_case_for_partner_shop_application_get() {
        let json = json!({
            "type": "EXISTING",
            "shopId": "f6f6c303-f70f-4f6e-bdc5-20b0d22f41a1",
        });

        let data: GetPartnerShopApplicationPayloadData =
            serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(
            data,
            GetPartnerShopApplicationPayloadData::Existing { .. }
        ));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_new_payload_when_using_camel_case_for_partner_shop_application_get() {
        let json = json!({
            "type": "NEW",
            "shopName": "Test Shop",
            "shopType": "COMMERCIAL_DEALER",
            "shopDomains": ["test.example"],
            "shopImage": "https://test.example/logo.svg",
        });

        let data: GetPartnerShopApplicationPayloadData =
            serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(
            data,
            GetPartnerShopApplicationPayloadData::New { .. }
        ));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_get_partner_shop_application_data_when_using_camel_case_fields() {
        let json = json!({
            "id": "0196580c-e4ca-723f-a7e0-1a73588380f0",
            "applicantUserId": "0196580c-e4ca-723f-a7e0-1a73588380f1",
            "businessState": "SUBMITTED",
            "executionState": "WAITING",
            "payload": {
                "type": "NEW",
                "shopName": "Test Shop",
                "shopType": "COMMERCIAL_DEALER",
                "shopDomains": ["test.example"],
                "shopImage": "https://test.example/logo.svg"
            },
            "created": "2026-04-22T00:00:00Z",
            "updated": "2026-04-22T01:00:00Z",
        });

        let data: GetPartnerShopApplicationData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
