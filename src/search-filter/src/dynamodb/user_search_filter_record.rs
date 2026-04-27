use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter_name::UserSearchFilterName;
use common::category_key::CategoryId;
use common::period_key::PeriodId;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::user_search_filter_id::UserSearchFilterId;
use common::year::Year;
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    price::domain::MonetaryAmount, product_state::domain::ProductState, user_id::UserId,
};
use geo::{core::continent::Continent, data::continent_data::ContinentData};
use isocountry::CountryCode;
use product::core::authenticity::Authenticity;
use product::core::condition::Condition;
use product::core::product_search::{GeoDistanceQuery, ProductSearch};
use product::core::provenance::Provenance;
use product::core::restoration::Restoration;
use product::dynamodb::authenticity_record::AuthenticityRecord;
use product::dynamodb::condition_record::ConditionRecord;
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::provenance_record::ProvenanceRecord;
use product::dynamodb::restoration_record::RestorationRecord;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::core::shop_type::ShopType;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct UserSearchFilterRecord {
    pub pk: String,
    pub sk: String,
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_search_description: Option<String>,

    #[serde(default = "default_notifications")]
    pub notifications: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_query: Option<TextQuery<1>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub category_id: HashSet<CategoryId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub period_id: HashSet<PeriodId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_shop_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub seller_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_seller_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_slug_id_query: HashSet<SlugId<0>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_shop_slug_id_query: HashSet<SlugId<0>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub seller_slug_id_query: HashSet<SlugId<0>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_seller_slug_id_query: HashSet<SlugId<0>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_type_query: HashSet<ShopTypeRecord>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub country_query: HashSet<CountryCode>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub continent_query: HashSet<ContinentData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address_distance_query: Option<GeoDistanceQuery>,
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
    pub origin_year_query: Option<RangeQuery<Year>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub authenticity_query: HashSet<AuthenticityRecord>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub condition_query: HashSet<ConditionRecord>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub provenance_query: HashSet<ProvenanceRecord>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub restoration_query: HashSet<RestorationRecord>,

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

fn default_notifications() -> bool {
    true
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
            enhanced_search_description: record.enhanced_search_description.map(Into::into),
            notifications: record.notifications,
            search: ProductSearch {
                language: record.language.into(),
                currency: record.currency.into(),
                product_query: record.product_query,
                category_id: record.category_id.into(),
                period_id: record.period_id.into(),
                shop_name_query: record.shop_name_query.into(),
                exclude_shop_name_query: record.exclude_shop_name_query.into(),
                seller_name_query: record.seller_name_query.into(),
                exclude_seller_name_query: record.exclude_seller_name_query.into(),
                shop_slug_id_query: record.shop_slug_id_query.into(),
                exclude_shop_slug_id_query: record.exclude_shop_slug_id_query.into(),
                seller_slug_id_query: record.seller_slug_id_query.into(),
                exclude_seller_slug_id_query: record.exclude_seller_slug_id_query.into(),
                shop_type_query: record
                    .shop_type_query
                    .into_iter()
                    .map(ShopType::from)
                    .collect(),
                country_query: record.country_query.into(),
                continent_query: record
                    .continent_query
                    .into_iter()
                    .map(Continent::from)
                    .collect(),
                geo_address_distance_query: record.geo_address_distance_query,
                price_query: record
                    .price_query
                    .map(|range_query| range_query.map(MonetaryAmount::from)),
                state_query: record
                    .state_query
                    .into_iter()
                    .map(ProductState::from)
                    .collect(),
                origin_year_query: record.origin_year_query,
                authenticity_query: record
                    .authenticity_query
                    .into_iter()
                    .map(Authenticity::from)
                    .collect(),
                condition_query: record
                    .condition_query
                    .into_iter()
                    .map(Condition::from)
                    .collect(),
                provenance_query: record
                    .provenance_query
                    .into_iter()
                    .map(Provenance::from)
                    .collect(),
                restoration_query: record
                    .restoration_query
                    .into_iter()
                    .map(Restoration::from)
                    .collect(),
                created_query: record.created_query,
                updated_query: record.updated_query,
                auction_start_query: record.auction_start_query,
                auction_end_query: record.auction_end_query,
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
            enhanced_search_description: user_search_filter
                .enhanced_search_description
                .map(Into::into),
            notifications: user_search_filter.notifications,
            product_query: user_search_filter.search.product_query,
            category_id: user_search_filter.search.category_id.into(),
            period_id: user_search_filter.search.period_id.into(),
            shop_name_query: user_search_filter.search.shop_name_query.into(),
            exclude_shop_name_query: user_search_filter.search.exclude_shop_name_query.into(),
            seller_name_query: user_search_filter.search.seller_name_query.into(),
            exclude_seller_name_query: user_search_filter.search.exclude_seller_name_query.into(),
            shop_slug_id_query: user_search_filter.search.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: user_search_filter.search.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: user_search_filter.search.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: user_search_filter
                .search
                .exclude_seller_slug_id_query
                .into(),
            shop_type_query: user_search_filter
                .search
                .shop_type_query
                .into_iter()
                .map(ShopTypeRecord::from)
                .collect(),
            country_query: user_search_filter.search.country_query.into(),
            continent_query: user_search_filter
                .search
                .continent_query
                .into_iter()
                .map(ContinentData::from)
                .collect(),
            geo_address_distance_query: user_search_filter.search.geo_address_distance_query,
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
            origin_year_query: user_search_filter.search.origin_year_query,
            authenticity_query: user_search_filter
                .search
                .authenticity_query
                .into_iter()
                .map(AuthenticityRecord::from)
                .collect(),
            condition_query: user_search_filter
                .search
                .condition_query
                .into_iter()
                .map(ConditionRecord::from)
                .collect(),
            provenance_query: user_search_filter
                .search
                .provenance_query
                .into_iter()
                .map(ProvenanceRecord::from)
                .collect(),
            restoration_query: user_search_filter
                .search
                .restoration_query
                .into_iter()
                .map(RestorationRecord::from)
                .collect(),
            auction_start_query: user_search_filter.search.auction_start_query,
            auction_end_query: user_search_filter.search.auction_end_query,
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
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id = config.fake_with_rng(rng);
            let search_filter_id = config.fake_with_rng(rng);
            UserSearchFilterRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&search_filter_id),
                user_id,
                user_search_filter_id: search_filter_id,
                name: config.fake_with_rng(rng),
                enhanced_search_description: config.fake_with_rng(rng),
                notifications: true,
                product_query: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                shop_name_query: config.fake_with_rng(rng),
                exclude_shop_name_query: config.fake_with_rng(rng),
                seller_name_query: config.fake_with_rng(rng),
                exclude_seller_name_query: config.fake_with_rng(rng),
                shop_slug_id_query: config.fake_with_rng(rng),
                exclude_shop_slug_id_query: config.fake_with_rng(rng),
                seller_slug_id_query: config.fake_with_rng(rng),
                exclude_seller_slug_id_query: config.fake_with_rng(rng),
                shop_type_query: config.fake_with_rng(rng),
                country_query: Default::default(),
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: None,
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: fake_range_query_datetime(config, rng),
                auction_end_query: fake_range_query_datetime(config, rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                origin_year_query: config.fake_with_rng(rng),
                authenticity_query: config.fake_with_rng(rng),
                condition_query: config.fake_with_rng(rng),
                provenance_query: config.fake_with_rng(rng),
                restoration_query: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}
