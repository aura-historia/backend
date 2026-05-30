use common::{
    actor::domain::Actor, product_id::ProductId, resource_state::domain::ResourceState, shop_id::ShopId,
    shops_product_id::ShopsProductId, user_id::UserId,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProduct {
    pub user_id: UserId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub product_id: ProductId,
    pub notifications: bool,
    pub state: ResourceState,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::core::watchlist_product::WatchlistProduct;
    use common::resource_state::domain::ResourceState;
    use fake::{Dummy, Fake, Faker, RngExt};
    use time::OffsetDateTime;

    impl Dummy<Faker> for WatchlistProduct {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            WatchlistProduct {
                shop_id: config.fake_with_rng(rng),
                user_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                product_id: config.fake_with_rng(rng),
                notifications: config.fake_with_rng(rng),
                state: ResourceState::Active,
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
