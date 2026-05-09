use common::{domain::Domain, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use shop::data::address_data::StructuredAddressData;
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPartnerShopApplicationData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name: Option<ShopName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_type: Option<ShopTypeData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_domains: Option<HashSet<Domain>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_url: Option<Url>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_image: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_structured_address: Option<StructuredAddressData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_email: Option<Email>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PatchPartnerShopApplicationData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchPartnerShopApplicationData {
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                shop_domains: config.fake_with_rng(rng),
                shop_url: config.fake_with_rng(rng),
                shop_image: config.fake_with_rng(rng),
                shop_structured_address: None,
                shop_phone: None,
                shop_email: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PatchPartnerShopApplicationData;
    use serde_json::json;

    #[test]
    fn should_roundtrip_patch_partner_shop_application_data_when_using_camel_case_fields() {
        let json = json!({
            "shopName": "Test Shop",
            "shopType": "COMMERCIAL_DEALER",
            "shopDomains": ["test.example"],
            "shopImage": "https://test.example/logo.svg",
        });

        let data: PatchPartnerShopApplicationData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
