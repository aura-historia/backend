use common::dynamodb_update::DynamoDbUpdate;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::{currency::record::CurrencyRecord, language::record::LanguageRecord};
use item::dynamodb::item_state_record::ItemStateRecord;
use search_filter_core::search_filter_name::SearchFilterName;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchFilterRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_filter_name: Option<SearchFilterName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_query: Option<TextQuery>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<TextQuery>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_query: Option<RangeQuery<u64>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_query: Option<HashSet<ItemStateRecord>>,

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for SearchFilterRecordUpdate {}

#[cfg(feature = "test-data")]
mod fake {
    use crate::search_filter_record_update::SearchFilterRecordUpdate;
    use fake::{Dummy, Fake, Faker};
    use search_filter_core::search_filter::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for SearchFilterRecordUpdate {
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            SearchFilterRecordUpdate {
                search_filter_name: config.fake_with_rng(rng),
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
