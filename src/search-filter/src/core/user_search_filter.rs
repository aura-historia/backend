use crate::{
    core::user_search_filter_id::UserSearchFilterId,
    core::user_search_filter_name::UserSearchFilterName,
};
use common::user_id::UserId;
use product::core::product_search::ProductSearch;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct UserSearchFilterSummary {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct UserSearchFilter {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub search: ProductSearch,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl From<UserSearchFilter> for UserSearchFilterSummary {
    fn from(filter: UserSearchFilter) -> Self {
        UserSearchFilterSummary {
            user_id: filter.user_id,
            user_search_filter_id: filter.user_search_filter_id,
            name: filter.name,
            created: filter.created,
            updated: filter.updated,
        }
    }
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

    impl Dummy<Faker> for UserSearchFilterSummary {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterSummary {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
