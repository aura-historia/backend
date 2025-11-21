use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::core::{
    user_search_filter::UserSearchFilter, user_search_filter_id::UserSearchFilterId,
};
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    price::domain::MonetaryAmount, product_state::domain::ProductState, user_id::UserId,
};
use product::core::product_search::ProductSearch;
use product::dynamodb::product_state_record::ProductStateRecord;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserSearchFilterRecord {
    pub pk: String,
    pub sk: String,
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub product_query: TextQuery,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<TextQuery>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub state_query: HashSet<ProductStateRecord>,

    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub created_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
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

pub fn mk_sk(search_filter_id: &UserSearchFilterId) -> String {
    format!("search_filter#{search_filter_id}")
}

impl From<UserSearchFilterRecord> for UserSearchFilter {
    fn from(record: UserSearchFilterRecord) -> Self {
        UserSearchFilter {
            user_id: record.user_id,
            user_search_filter_id: record.user_search_filter_id,
            name: record.name,
            search: ProductSearch {
                language: record.language.into(),
                currency: record.currency.into(),
                product_query: record.product_query,
                shop_name_query: record.shop_name_query,
                price_query: record
                    .price_query
                    .map(|range_query| range_query.map(MonetaryAmount::from)),
                state_query: record
                    .state_query
                    .into_iter()
                    .map(ProductState::from)
                    .collect(),
                created_query: record.created_query,
                updated_query: record.updated_query,
            },
            created: record.created,
            updated: record.updated,
        }
    }
}

impl From<UserSearchFilter> for UserSearchFilterRecord {
    fn from(user_search_filter: UserSearchFilter) -> Self {
        UserSearchFilterRecord {
            pk: mk_pk(&user_search_filter.user_id),
            sk: mk_sk(&user_search_filter.user_search_filter_id),
            user_id: user_search_filter.user_id,
            user_search_filter_id: user_search_filter.user_search_filter_id,
            name: user_search_filter.name,
            product_query: user_search_filter.search.product_query,
            shop_name_query: user_search_filter.search.shop_name_query,
            price_query: user_search_filter
                .search
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: user_search_filter
                .search
                .state_query
                .into_iter()
                .map(ProductStateRecord::from)
                .collect(),
            created_query: user_search_filter.search.created_query,
            language: user_search_filter.search.language.into(),
            currency: user_search_filter.search.currency.into(),
            updated_query: user_search_filter.search.updated_query,
            created: user_search_filter.created,
            updated: user_search_filter.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::dynamodb::user_search_filter_record::{UserSearchFilterRecord, mk_pk, mk_sk};
    use fake::{Dummy, Fake, Faker};
    use product::core::product_search::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for UserSearchFilterRecord {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id = config.fake_with_rng(rng);
            let search_filter_id = config.fake_with_rng(rng);
            UserSearchFilterRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&search_filter_id),
                user_id,
                user_search_filter_id: search_filter_id,
                name: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
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
