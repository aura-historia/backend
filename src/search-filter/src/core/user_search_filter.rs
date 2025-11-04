use crate::{
    core::user_search_filter_id::UserSearchFilterId,
    core::user_search_filter_name::UserSearchFilterName,
};
use common::user_id::UserId;
use item::core::item_search::ItemSearch;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct UserSearchFilter {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub search: ItemSearch,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for UserSearchFilter {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilter {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                search: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
