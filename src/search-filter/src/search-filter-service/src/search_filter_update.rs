use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    item_state::domain::ItemState,
    language::{domain::Language, record::LanguageRecord},
    price::domain::MonetaryAmount,
};
use item_dynamodb::item_state_record::ItemStateRecord;
use search_filter_dynamodb::search_filter_record_update::SearchFilterRecordUpdate;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterUpdate {
    pub item_query: Option<TextQuery>,
    pub shop_name_query: Option<TextQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: Option<AnyOfQuery<ItemState>>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub updated: OffsetDateTime,
}

impl SearchFilterUpdate {
    pub fn is_empty(&self) -> bool {
        let SearchFilterUpdate {
            item_query,
            shop_name_query,
            price_query,
            state_query,
            created_query,
            updated_query,
            language,
            currency,
            updated: _,
        } = self;

        item_query.is_none()
            && shop_name_query.is_none()
            && price_query.is_none()
            && state_query.is_none()
            && created_query.is_none()
            && updated_query.is_none()
            && language.is_none()
            && currency.is_none()
    }
}

impl From<SearchFilterUpdate> for SearchFilterRecordUpdate {
    fn from(update: SearchFilterUpdate) -> Self {
        SearchFilterRecordUpdate {
            item_query: update.item_query,
            shop_name_query: update.shop_name_query,
            price_query: update
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: update
                .state_query
                .map(|states| states.into_iter().map(ItemStateRecord::from).collect()),
            created_query: update.created_query,
            updated_query: update.updated_query,
            language: update.language.map(LanguageRecord::from),
            currency: update.currency.map(CurrencyRecord::from),
            updated: update.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::search_filter_update::SearchFilterUpdate;
    use fake::{Dummy, Fake, Faker};
    use search_filter_core::search_filter::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for SearchFilterUpdate {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilterUpdate {
                item_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
