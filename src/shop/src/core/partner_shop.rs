use crate::core::{partner_shop_api_key::HashedPartnerShopApiKey, shop_type::ShopType};
use common::{
    domain::Domain, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId, user_id::UserId,
};
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
    pub image: Option<Url>,
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
                image: config.fake_with_rng(rng),
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
