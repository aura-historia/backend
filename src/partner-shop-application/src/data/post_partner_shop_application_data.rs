use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum PostPartnerShopApplicationPayloadData {
    #[serde(rename = "EXISTING")]
    Existing { shop_id: ShopId },
    #[serde(rename = "NEW")]
    New {
        shop_name: ShopName,
        shop_type: ShopTypeData,
        shop_domains: HashSet<Domain>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        shop_image: Option<Url>,
    },
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PostPartnerShopApplicationPayloadData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            if config.fake_with_rng::<bool, R>(rng) {
                PostPartnerShopApplicationPayloadData::Existing {
                    shop_id: config.fake_with_rng(rng),
                }
            } else {
                PostPartnerShopApplicationPayloadData::New {
                    shop_name: config.fake_with_rng(rng),
                    shop_type: config.fake_with_rng(rng),
                    shop_domains: config.fake_with_rng(rng),
                    shop_image: config.fake_with_rng(rng),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PostPartnerShopApplicationPayloadData;
    use serde_json::json;

    #[test]
    fn should_roundtrip_existing_payload_when_using_camel_case_for_partner_shop_application_post() {
        let json = json!({
            "type": "EXISTING",
            "shopId": "f6f6c303-f70f-4f6e-bdc5-20b0d22f41a1",
        });

        let data: PostPartnerShopApplicationPayloadData =
            serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(
            data,
            PostPartnerShopApplicationPayloadData::Existing { .. }
        ));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_new_payload_when_using_camel_case_for_partner_shop_application_post() {
        let json = json!({
            "type": "NEW",
            "shopName": "Test Shop",
            "shopType": "COMMERCIAL_DEALER",
            "shopDomains": ["test.example"],
            "shopImage": "https://test.example/logo.svg",
        });

        let data: PostPartnerShopApplicationPayloadData =
            serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(
            data,
            PostPartnerShopApplicationPayloadData::New { .. }
        ));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
