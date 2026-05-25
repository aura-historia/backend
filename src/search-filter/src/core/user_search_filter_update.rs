use crate::core::user_search_filter::EnhancedSearchDescription;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate;
use common::distance::domain::GeoDistanceQuery;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::domain::ResourceState;
use common::resource_state::record::ResourceStateRecord;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use common::{
    currency::{domain::Currency, record::CurrencyRecord},
    language::{domain::Language, record::LanguageRecord},
    price::domain::MonetaryAmount,
    product_state::domain::ProductState,
};
use geo::core::continent::Continent;
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use product::dynamodb::product_state_record::ProductStateRecord;
use shop::core::shop_type::ShopType;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct UserSearchFilterUpdate {
    pub name: Option<UserSearchFilterName>,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub notifications: Option<bool>,
    pub state: Option<ResourceState>,
    pub product_query: Option<TextQuery<1>>,
    pub shop_name_query: Option<HashSet<ShopName>>,
    pub exclude_shop_name_query: Option<HashSet<ShopName>>,
    pub seller_name_query: Option<HashSet<ShopName>>,
    pub exclude_seller_name_query: Option<HashSet<ShopName>>,
    pub shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub exclude_shop_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub exclude_seller_slug_id_query: Option<HashSet<SlugId<0>>>,
    pub shop_type_query: Option<AnyOfQuery<ShopType>>,
    pub country_query: Option<AnyOfQuery<CountryCode>>,
    pub continent_query: Option<AnyOfQuery<Continent>>,
    pub geo_address_distance_query: Option<GeoDistanceQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: Option<AnyOfQuery<ProductState>>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_start_query: Option<RangeQuery<OffsetDateTime>>,
    pub auction_end_query: Option<RangeQuery<OffsetDateTime>>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: Option<OffsetDateTime>,
}

impl Default for UserSearchFilterUpdate {
    fn default() -> Self {
        Self {
            name: None,
            enhanced_search_description: None,
            notifications: None,
            state: None,
            product_query: None,
            shop_name_query: None,
            exclude_shop_name_query: None,
            seller_name_query: None,
            exclude_seller_name_query: None,
            shop_slug_id_query: None,
            exclude_shop_slug_id_query: None,
            seller_slug_id_query: None,
            exclude_seller_slug_id_query: None,
            shop_type_query: None,
            country_query: None,
            continent_query: None,
            geo_address_distance_query: None,
            price_query: None,
            state_query: None,
            created_query: None,
            updated_query: None,
            auction_start_query: None,
            auction_end_query: None,
            language: None,
            currency: None,
            updated: OffsetDateTime::now_utc(),
            last_hybrid_search_matched: None,
        }
    }
}

impl UserSearchFilterUpdate {
    pub fn is_empty(&self) -> bool {
        let UserSearchFilterUpdate {
            name,
            enhanced_search_description,
            notifications,
            state,
            product_query,
            shop_name_query,
            exclude_shop_name_query,
            seller_name_query,
            exclude_seller_name_query,
            shop_slug_id_query,
            exclude_shop_slug_id_query,
            seller_slug_id_query,
            exclude_seller_slug_id_query,
            shop_type_query,
            country_query,
            continent_query,
            geo_address_distance_query,
            price_query,
            state_query,
            created_query,
            updated_query,
            auction_start_query,
            auction_end_query,
            language,
            currency,
            updated: _,
            last_hybrid_search_matched,
        } = self;

        name.is_none()
            && enhanced_search_description.is_none()
            && notifications.is_none()
            && state.is_none()
            && product_query.is_none()
            && shop_name_query.is_none()
            && exclude_shop_name_query.is_none()
            && seller_name_query.is_none()
            && exclude_seller_name_query.is_none()
            && shop_slug_id_query.is_none()
            && exclude_shop_slug_id_query.is_none()
            && seller_slug_id_query.is_none()
            && exclude_seller_slug_id_query.is_none()
            && shop_type_query.is_none()
            && country_query.is_none()
            && continent_query.is_none()
            && geo_address_distance_query.is_none()
            && price_query.is_none()
            && state_query.is_none()
            && created_query.is_none()
            && updated_query.is_none()
            && auction_start_query.is_none()
            && auction_end_query.is_none()
            && language.is_none()
            && currency.is_none()
            && last_hybrid_search_matched.is_none()
    }
}

impl From<UserSearchFilterUpdate> for UserSearchFilterRecordUpdate {
    fn from(update: UserSearchFilterUpdate) -> Self {
        UserSearchFilterRecordUpdate {
            name: update.name,
            notifications: update.notifications,
            state: update.state.map(ResourceStateRecord::from),
            enhanced_search_description: update.enhanced_search_description.map(Into::into),
            product_query: update.product_query,
            shop_name_query: update.shop_name_query,
            exclude_shop_name_query: update.exclude_shop_name_query,
            seller_name_query: update.seller_name_query,
            exclude_seller_name_query: update.exclude_seller_name_query,
            shop_slug_id_query: update.shop_slug_id_query,
            exclude_shop_slug_id_query: update.exclude_shop_slug_id_query,
            seller_slug_id_query: update.seller_slug_id_query,
            exclude_seller_slug_id_query: update.exclude_seller_slug_id_query,
            shop_type_query: update
                .shop_type_query
                .map(|types| types.into_iter().map(ShopTypeRecord::from).collect()),
            country_query: update.country_query.map(Into::into),
            continent_query: update
                .continent_query
                .map(|continents| continents.into_iter().map(ContinentData::from).collect()),
            geo_address_distance_query: update.geo_address_distance_query.map(Into::into),
            price_query: update
                .price_query
                .map(|range_query| range_query.map(u64::from)),
            state_query: update
                .state_query
                .map(|states| states.into_iter().map(ProductStateRecord::from).collect()),
            created_query: update.created_query,
            updated_query: update.updated_query,
            auction_start_query: update.auction_start_query,
            auction_end_query: update.auction_end_query,
            language: update.language.map(LanguageRecord::from),
            currency: update.currency.map(CurrencyRecord::from),
            updated: update.updated,
            last_hybrid_search_matched: update.last_hybrid_search_matched,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user_search_filter_update::UserSearchFilterUpdate;
    use fake::{Dummy, Fake, Faker};
    use product::core::product_search::faker::fake_range_query_datetime;
    use time::OffsetDateTime;

    impl Dummy<Faker> for UserSearchFilterUpdate {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterUpdate {
                name: config.fake_with_rng(rng),
                enhanced_search_description: config.fake_with_rng(rng),
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
                country_query: None,
                continent_query: config.fake_with_rng(rng),
                geo_address_distance_query: config.fake_with_rng(rng),
                price_query: config.fake_with_rng(rng),
                state_query: config.fake_with_rng(rng),
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                auction_start_query: config.fake_with_rng(rng),
                auction_end_query: config.fake_with_rng(rng),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                updated: OffsetDateTime::now_utc(),
                last_hybrid_search_matched: Some(OffsetDateTime::now_utc()),
            }
        }
    }
}
