use crate::data::address_data::StructuredAddressData;
use crate::data::shop_type_data::ShopTypeData;
use common::{domain::Domain, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostShopData {
    pub name: ShopName,
    pub shop_type: ShopTypeData,
    pub domains: HashSet<Domain>,
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
    use fake::{Dummy, Fake, Faker};

    impl Dummy<Faker> for PostShopData {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostShopData {
                name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                domains: vec![config.fake_with_rng(rng)].into_iter().collect(),
                shopify_domain: config.fake_with_rng(rng),
                url: None,
                image: None,
                structured_address: None,
                phone: None,
                email: None,
            }
        }
    }
}
