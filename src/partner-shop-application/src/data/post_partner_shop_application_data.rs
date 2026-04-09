use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName};
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
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
