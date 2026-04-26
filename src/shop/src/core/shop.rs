use crate::core::{
    address::{GeoAddress, StructuredAddress},
    partner_shop::PartnerShop,
    partner_status::ShopPartnerStatus,
    shop_type::ShopType,
};
use common::{
    category_key::CategoryId, domain::Domain, period_key::PeriodId, shop_id::ShopId,
    shop_name::ShopName, slug_id::SlugId,
};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Shop {
    pub shop_id: ShopId,
    pub shop_slug_id: SlugId<0>,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub specialities_categories: Vec<CategoryId>,
    pub specialities_periods: Vec<PeriodId>,
    pub partner_status: ShopPartnerStatus,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl From<PartnerShop> for Shop {
    fn from(partner_shop: PartnerShop) -> Self {
        Shop {
            shop_id: partner_shop.shop_id,
            shop_slug_id: partner_shop.shop_slug_id,
            name: partner_shop.name,
            shop_type: partner_shop.shop_type,
            domains: partner_shop.domains,
            image: partner_shop.image,
            structured_address: partner_shop.structured_address,
            geo_address: partner_shop.geo_address,
            phone: partner_shop.phone,
            email: partner_shop.email,
            specialities_categories: partner_shop.specialities_categories,
            specialities_periods: partner_shop.specialities_periods,
            partner_status: ShopPartnerStatus::Partnered,
            created: partner_shop.created,
            updated: partner_shop.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for Shop {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let name: ShopName = config.fake_with_rng(rng);
            Shop {
                shop_id: config.fake_with_rng(rng),
                shop_slug_id: SlugId::from(name.as_ref()),
                name,
                shop_type: config.fake_with_rng(rng),
                domains: [Faker.fake()].into(),
                image: config.fake_with_rng(rng),
                structured_address: None,
                geo_address: None,
                phone: None,
                email: None,
                specialities_categories: Vec::new(),
                specialities_periods: Vec::new(),
                partner_status: config.fake_with_rng(rng),
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
