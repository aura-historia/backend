use crate::{any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery};
use common::currency::domain::Currency;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::price::domain::MonetaryAmount;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub language: Language,
    pub currency: Currency,
    pub item_query: TextQuery,
    pub shop_name_query: Option<TextQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: AnyOfQuery<ItemState>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for SearchFilter {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilter {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                item_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
            }
        }
    }

    pub fn fake_range_query_datetime<R: fake::Rng + ?Sized>(
        config: &Faker,
        rng: &mut R,
    ) -> Option<RangeQuery<OffsetDateTime>> {
        if config.fake_with_rng(rng) {
            None
        } else {
            let min = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            let max = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            Some(RangeQuery { min, max })
        }
    }
}
