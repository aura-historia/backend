use crate::data::partner_shop_application_state_data::PartnerShopApplicationStateData;
use common::{domain::Domain, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPartnerShopApplicationData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PartnerShopApplicationStateData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name: Option<ShopName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_type: Option<ShopTypeData>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_domains: Option<HashSet<Domain>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_image: Option<Url>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PatchPartnerShopApplicationData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PatchPartnerShopApplicationData {
                state: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                shop_domains: config.fake_with_rng(rng),
                shop_image: config.fake_with_rng(rng),
            }
        }
    }
}
