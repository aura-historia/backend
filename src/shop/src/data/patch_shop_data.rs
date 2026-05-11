use crate::data::address_data::StructuredAddressData;
use crate::data::shop_type_data::ShopTypeData;
use common::domain::Domain;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchShopData {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub domains: Option<HashSet<Domain>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shopify_domain: Option<Domain>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PatchShopData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchShopData {
                shop_type: config.fake_with_rng(rng),
                domains: config.fake_with_rng(rng),
                shopify_domain: config.fake_with_rng(rng),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                phone: None,
                email: None,
            }
        }
    }
}
