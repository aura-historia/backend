use crate::core::{
    address::{GeoAddress, StructuredAddress},
    partner_shop_api_key::HashedPartnerShopApiKey,
    shop_type::ShopType,
};
use common::{
    category_key::CategoryId, domain::Domain, period_key::PeriodId, shop_id::ShopId,
    shop_name::ShopName, slug_id::SlugId, user_id::UserId,
};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShop {
    pub hashed_api_key: Option<HashedPartnerShopApiKey>,
    pub shop_id: ShopId,
    pub shop_slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub specialities_categories: Vec<CategoryId>,
    pub specialities_periods: Vec<PeriodId>,
    pub partner_user_id: UserId,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PartnerShop {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let name: ShopName = config.fake_with_rng(rng);
            PartnerShop {
                hashed_api_key: Some(config.fake_with_rng(rng)),
                shop_id: config.fake_with_rng(rng),
                shop_slug_id: SlugId::from(name.as_ref()),
                name,
                shop_type: config.fake_with_rng(rng),
                domains: [Faker.fake()].into(),
                url: config.fake_with_rng(rng),
                image: config.fake_with_rng(rng),
                structured_address: None,
                geo_address: None,
                phone: None,
                email: None,
                specialities_categories: Vec::new(),
                specialities_periods: Vec::new(),
                partner_user_id: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::shop::Shop;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop() {
            let _ = Faker.fake::<Shop>();
        }
    }
}
