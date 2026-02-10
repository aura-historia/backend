use crate::core::user_search_filter_name::UserSearchFilterName;
use common::category_key::CategoryId;
use common::dynamodb_update::DynamoDbUpdate;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::year::Year;
use common::{currency::record::CurrencyRecord, language::record::LanguageRecord};
use product::dynamodb::authenticity_record::AuthenticityRecord;
use product::dynamodb::condition_record::ConditionRecord;
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::provenance_record::ProvenanceRecord;
use product::dynamodb::restoration_record::RestorationRecord;
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
    pub product_query: Option<TextQuery<3>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<CategoryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name_query: Option<HashSet<ShopName>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_year_query: Option<RangeQuery<Year>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authenticity_query: Option<HashSet<AuthenticityRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_query: Option<HashSet<ConditionRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_query: Option<HashSet<ProvenanceRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoration_query: Option<HashSet<RestorationRecord>>,

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
        fn dummy_with_rng<R: fake::Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterRecordUpdate {
                name: config.fake_with_rng(rng),
                product_query: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
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
        assert!(
            UserSearchFilterRecordUpdate::SERDE_FIELDS
                .iter()
                .all(|field| UserSearchFilterRecord::SERDE_FIELDS.contains(field))
        )
    }
}
