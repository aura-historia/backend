use crate::core::user_search_filter_name::UserSearchFilterName;
use common::dynamodb_update::DynamoDbUpdate;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::record::ResourceStateRecord;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::{currency::record::CurrencyRecord, language::record::LanguageRecord};
use product::dynamodb::product_state_record::ProductStateRecord;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserSearchFilterRecordUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<UserSearchFilterName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ResourceStateRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_query: Option<TextQuery<1>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<HashSet<ShopName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_shop_name_query: Option<HashSet<ShopName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seller_name_query: Option<HashSet<ShopName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_seller_name_query: Option<HashSet<ShopName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_type_query: Option<HashSet<ShopTypeRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_query: Option<RangeQuery<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_query: Option<HashSet<ProductStateRecord>>,
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

    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "common::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<LanguageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CurrencyRecord>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl DynamoDbUpdate for UserSearchFilterRecordUpdate {}

#[cfg(feature = "test-data")]
mod fake {
    use crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate;
    use fake::{Dummy, Fake, Faker};
    use product::core::product_search::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for UserSearchFilterRecordUpdate {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterRecordUpdate {
                name: config.fake_with_rng(rng),
                notifications: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: fake_range_query_datetime(config, rng),
                auction_end_query: fake_range_query_datetime(config, rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb::{
        user_search_filter_record::UserSearchFilterRecord,
        user_search_filter_record_update::UserSearchFilterRecordUpdate,
    };

    #[test]
    fn should_be_subset_of_user_search_filter_record() {
        assert!(UserSearchFilterRecordUpdate::SERDE_FIELDS
            .iter()
            .all(|field| UserSearchFilterRecord::SERDE_FIELDS.contains(field)))
    }
}
