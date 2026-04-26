use crate::data::address_data::StructuredAddressData;
use crate::data::shop_type_data::ShopTypeData;
use common::{category_key::CategoryId, domain::Domain, period_key::PeriodId};
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
    pub image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<Email>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub specialities_categories: Option<Vec<CategoryId>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub specialities_periods: Option<Vec<PeriodId>>,
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
                image: config.fake_with_rng(rng),
                structured_address: None,
                phone: None,
                email: None,
                specialities_categories: None,
                specialities_periods: None,
            }
        }
    }
}
