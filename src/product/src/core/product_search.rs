use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::price::domain::MonetaryAmount;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::year::Year;
use shop::core::shop_type::ShopType;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSearch {
    pub language: Language,
    pub currency: Currency,
    pub product_query: TextQuery,
    pub shop_name_query: AnyOfQuery<ShopName>,
    pub exclude_shop_name_query: AnyOfQuery<ShopName>,
    pub shop_type_query: AnyOfQuery<ShopType>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: AnyOfQuery<ProductState>,
    pub origin_year_query: Option<RangeQuery<Year>>,
    pub authenticity_query: AnyOfQuery<Authenticity>,
    pub condition_query: AnyOfQuery<Condition>,
    pub provenance_query: AnyOfQuery<Provenance>,
    pub restoration_query: AnyOfQuery<Restoration>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductSearch {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductSearch {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: fake_range_query_datetime(config, rng),
                auction_end_query: fake_range_query_datetime(config, rng),
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
