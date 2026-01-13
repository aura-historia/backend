use common::{domain::Domain, shop_id::ShopId, shop_name::ShopName};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

use crate::core::shop_type::ShopType;

#[derive(Debug, Clone, PartialEq)]
pub struct Shop {
    pub shop_id: ShopId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub image: Option<Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for Shop {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Shop {
                shop_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                domains: [Faker.fake()].into(),
                image: config.fake_with_rng(rng),
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
