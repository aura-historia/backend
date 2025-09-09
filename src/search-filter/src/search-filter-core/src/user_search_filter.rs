use crate::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use common::user_id::UserId;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct UserSearchFilter {
    pub user_id: UserId,
    pub search_filter_id: SearchFilterId,
    pub search_filter: SearchFilter,
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
                search_filter_id: config.fake_with_rng(rng),
                search_filter: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
