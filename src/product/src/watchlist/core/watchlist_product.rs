use crate::core::item::LocalizedItemView;
use common::{product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedWatchlistItemView {
    pub item: LocalizedItemView,
    pub notifications: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistItem {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,
    pub notifications: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::watchlist::core::watchlist_product::{LocalizedWatchlistItemView, WatchlistItem};
    use fake::{Dummy, Fake, Faker, Rng};
    use time::OffsetDateTime;

    impl Dummy<Faker> for LocalizedWatchlistItemView {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedWatchlistItemView {
                item: config.fake_with_rng(rng),
                notifications: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for WatchlistItem {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            WatchlistItem {
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
