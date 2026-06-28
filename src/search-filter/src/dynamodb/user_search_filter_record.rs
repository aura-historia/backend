use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter_name::UserSearchFilterName;
use common::actor::record::ActorRecord;
use common::distance::data::GeoDistanceQueryData;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::record::ResourceStateRecord;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::user_search_filter_id::UserSearchFilterId;
use common::{
    currency::record::CurrencyRecord, language::record::LanguageRecord,
    price::domain::MonetaryAmount, product_state::domain::ProductState, user_id::UserId,
};
use geo::{core::continent::Continent, data::continent_data::ContinentData};
use isocountry::CountryCode;
use product::core::product_search::ProductSearch;
use product::dynamodb::product_state_record::ProductStateRecord;
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

    #[serde(default = "default_notifications")]
    pub notifications: bool,
    #[serde(default)]
    pub state: ResourceStateRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub product_query: Vec<TextQuery<1>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_search_description: Option<String>,
    // dim=768 via google/gemini-embedding-2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_shop_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub seller_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_seller_name_query: HashSet<ShopName>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub exclude_seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub shop_type_query: HashSet<ShopTypeRecord>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub country_query: HashSet<CountryCode>,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub continent_query: HashSet<ContinentData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_address_distance_query: Option<GeoDistanceQueryData>,
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

    pub language: LanguageRecord,
    pub currency: CurrencyRecord,
    pub created_by: ActorRecord,
    pub updated_by: ActorRecord,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(
        with = "time::serde::rfc3339",
        default = "default_last_hybrid_search_matched"
    )]
    pub last_hybrid_search_matched: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

fn default_notifications() -> bool {
    true
}

fn default_last_hybrid_search_matched() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
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
            notifications: record.notifications,
            state: record.state.into(),
            search: ProductSearch {
                language: record.language.into(),
                currency: record.currency.into(),
                product_query: record.product_query,
                enhanced_search_description: record.enhanced_search_description.map(Into::into),
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
                geo_address_distance_query: record.geo_address_distance_query.map(Into::into),
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
                auction_start_query: record.auction_start_query,
                auction_end_query: record.auction_end_query,
            },
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
            last_hybrid_search_matched: record.last_hybrid_search_matched,
            embedding: record.embedding,
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
            notifications: user_search_filter.notifications,
            state: user_search_filter.state.into(),
            product_query: user_search_filter.search.product_query,
            enhanced_search_description: user_search_filter
                .search
                .enhanced_search_description
                .map(Into::into),
            embedding: user_search_filter.embedding,
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
            geo_address_distance_query: user_search_filter
                .search
                .geo_address_distance_query
                .map(Into::into),
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
            auction_start_query: user_search_filter.search.auction_start_query,
            auction_end_query: user_search_filter.search.auction_end_query,
            created_by: user_search_filter.created_by.into(),
            updated_by: user_search_filter.updated_by.into(),
            created: user_search_filter.created,
            updated: user_search_filter.updated,
            last_hybrid_search_matched: user_search_filter.last_hybrid_search_matched,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::dynamodb::user_search_filter_record::{UserSearchFilterRecord, mk_pk, mk_sk};
    use common::resource_state::record::ResourceStateRecord;
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
                notifications: true,
                state: ResourceStateRecord::Active,
                product_query: config.fake_with_rng(rng),
                enhanced_search_description: config.fake_with_rng(rng),
                embedding: None,
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
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
                last_hybrid_search_matched: OffsetDateTime::now_utc(),
            }
        }
    }
}
