use item_core::item::LocalizedItemView;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedWatchlistItemView {
    pub item: LocalizedItemView,
    pub created: OffsetDateTime,
    pub notifications: bool,
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::domain::LocalizedWatchlistItemView;
    use fake::{Dummy, Fake, Faker, Rng};
    use time::OffsetDateTime;

    impl Dummy<Faker> for LocalizedWatchlistItemView {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedWatchlistItemView {
                item: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                notifications: config.fake_with_rng(rng),
            }
        }
    }
}
