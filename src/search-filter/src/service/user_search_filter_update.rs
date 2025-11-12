use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    product_state::domain::ProductState,
    language::{domain::Language, record::LanguageRecord},
    price::domain::MonetaryAmount,
};
use product::dynamodb::item_state_record::ProductStateRecord;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct UserSearchFilterUpdate {
    pub name: Option<UserSearchFilterName>,
    pub item_query: Option<TextQuery>,
    pub shop_name_query: Option<TextQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: Option<AnyOfQuery<ProductState>>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub updated: OffsetDateTime,
}

impl UserSearchFilterUpdate {
    pub fn is_empty(&self) -> bool {
        let UserSearchFilterUpdate {
            name: search_filter_name,
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

        search_filter_name.is_none()
            && item_query.is_none()
            && shop_name_query.is_none()
            && price_query.is_none()
            && state_query.is_none()
            && created_query.is_none()
            && updated_query.is_none()
            && language.is_none()
            && currency.is_none()
    }
}

impl From<UserSearchFilterUpdate> for UserSearchFilterRecordUpdate {
    fn from(update: UserSearchFilterUpdate) -> Self {
        UserSearchFilterRecordUpdate {
            name: update.name,
            item_query: update.item_query,
            shop_name_query: update.shop_name_query,
            price_query: update
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: update
                .state_query
                .map(|states| states.into_iter().map(ProductStateRecord::from).collect()),
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
    use crate::service::user_search_filter_update::UserSearchFilterUpdate;
    use fake::{Dummy, Fake, Faker};
    use product::core::item_search::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for UserSearchFilterUpdate {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterUpdate {
                name: config.fake_with_rng(rng),
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
