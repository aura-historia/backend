use common::{product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProduct {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,
    pub notifications: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::core::watchlist_product::WatchlistProduct;
    use fake::{Dummy, Fake, Faker, RngExt};
    use time::OffsetDateTime;

    impl Dummy<Faker> for WatchlistProduct {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            WatchlistProduct {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                product_id: config.fake_with_rng(rng),
                notifications: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
