use common::{
    currency::record::CurrencyRecord, item_state::domain::ItemState,
    language::record::LanguageRecord, price::domain::MonetaryAmount, user_id::UserId,
};
use item_dynamodb::item_state_record::ItemStateRecord;
use search_filter_core::{
    range_query::RangeQuery, search_filter::SearchFilter, search_filter_id::SearchFilterId,
    text_query::TextQuery, user_search_filter::UserSearchFilter,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchFilterRecord {
    pub pk: String,

    pub sk: String,

    pub user_id: UserId,

    pub search_filter_id: SearchFilterId,

    pub item_query: TextQuery,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<TextQuery>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub state_query: HashSet<ItemStateRecord>,

    #[serde(
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        with = "search_filter_core::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,

    pub language: LanguageRecord,

    pub currency: CurrencyRecord,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(search_filter_id: &SearchFilterId) -> String {
    format!("search_filter#{search_filter_id}")
}

impl From<SearchFilterRecord> for UserSearchFilter {
    fn from(record: SearchFilterRecord) -> Self {
        UserSearchFilter {
            user_id: record.user_id,
            search_filter_id: record.search_filter_id,
            search_filter: SearchFilter {
                language: record.language.into(),
                currency: record.currency.into(),
                item_query: record.item_query,
                shop_name_query: record.shop_name_query,
                price_query: record
                    .price_query
                    .map(|range_query| range_query.map(MonetaryAmount::from)),
                state_query: record
                    .state_query
                    .into_iter()
                    .map(ItemState::from)
                    .collect(),
                created_query: record.created_query,
                updated_query: record.updated_query,
            },
            created: record.created,
            updated: record.updated,
        }
    }
}

impl From<UserSearchFilter> for SearchFilterRecord {
    fn from(user_search_filter: UserSearchFilter) -> Self {
        SearchFilterRecord {
            pk: mk_pk(&user_search_filter.user_id),
            sk: mk_sk(&user_search_filter.search_filter_id),
            user_id: user_search_filter.user_id,
            search_filter_id: user_search_filter.search_filter_id,
            item_query: user_search_filter.search_filter.item_query,
            shop_name_query: user_search_filter.search_filter.shop_name_query,
            price_query: user_search_filter
                .search_filter
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: user_search_filter
                .search_filter
                .state_query
                .into_iter()
                .map(ItemStateRecord::from)
                .collect(),
            created_query: user_search_filter.search_filter.created_query,
            language: user_search_filter.search_filter.language.into(),
            currency: user_search_filter.search_filter.currency.into(),
            updated_query: user_search_filter.search_filter.updated_query,
            created: user_search_filter.created,
            updated: user_search_filter.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::search_filter_record::{SearchFilterRecord, mk_pk, mk_sk};
    use fake::{Dummy, Fake, Faker};
    use search_filter_core::search_filter::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for SearchFilterRecord {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id = config.fake_with_rng(rng);
            let search_filter_id = config.fake_with_rng(rng);
            SearchFilterRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&search_filter_id),
                user_id,
                search_filter_id,
                item_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
