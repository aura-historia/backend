use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::document::ResourceStateDocument;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use geo::{
    core::distance::{Distance, DistanceUnit, GeoDistanceQuery},
    data::continent_data::ContinentData,
};
use isocountry::CountryCode;
use localization::Language;
use money::{Currency, MonetaryAmount};
use product_core::product_id::ProductId;
use product_core::product_lifecycle::ProductLifecycle;
use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use product_core::product_state::ProductState;
use product_opensearch::build_percolator_query;
use search_filter_core::ResourceState;
use search_filter_service::ports::{SearchFilterProjection, SearchFilterView};
use serde::ser::Error as _;
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
use shop_core::{seller_slug_id::SellerSlugId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use std::collections::HashSet;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PRODUCT_SEARCH_FIELDS: [&str; 24] = [
    "language",
    "currency",
    "productQuery",
    "enhancedSearchDescription",
    "excludeProductId",
    "shopName",
    "excludeShopName",
    "sellerName",
    "excludeSellerName",
    "shopSlugId",
    "excludeShopSlugId",
    "sellerSlugId",
    "excludeSellerSlugId",
    "shopType",
    "country",
    "continent",
    "geoAddress",
    "price",
    "state",
    "lifecycle",
    "created",
    "updated",
    "auctionStart",
    "auctionEnd",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: ResourceStateDocument,
    pub source_version: i64,
    pub search: serde_json::Value,
    pub query: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_hybrid_search_matched: OffsetDateTime,
}

/// Decode failure for the complete product-search payload stored in a search document.
#[derive(Debug, thiserror::Error)]
pub enum ProductSearchDocumentMappingError {
    #[error("OpenSearch document has malformed product search JSON")]
    Deserialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("OpenSearch document has an invalid product search timestamp")]
    InvalidTimestamp,
}

impl TryFrom<&SearchFilterProjection> for SearchFilterDocument {
    type Error = serde_json::Error;

    fn try_from(projection: &SearchFilterProjection) -> Result<Self, Self::Error> {
        let view = &projection.view;
        Ok(Self {
            user_search_filter_id: view.search_filter_id,
            user_id: view.user_id,
            name: view.name.clone(),
            notifications: view.notifications,
            state: state_to_document(view.state),
            source_version: projection.source_version,
            search: product_search_to_value(&view.search)?,
            query: build_percolator_query(&view.search)?,
            embedding: view.embedding.clone(),
            created: view.created,
            updated: view.updated,
            last_hybrid_search_matched: view.last_hybrid_search_matched,
        })
    }
}

impl TryFrom<SearchFilterDocument> for SearchFilterView {
    type Error = ProductSearchDocumentMappingError;

    fn try_from(document: SearchFilterDocument) -> Result<Self, Self::Error> {
        Ok(SearchFilterView {
            search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            notifications: document.notifications,
            state: state_from_document(document.state),
            search: product_search_from_value(document.search)?,
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
            last_hybrid_search_matched: document.last_hybrid_search_matched,
        })
    }
}

pub(crate) fn state_to_document(state: ResourceState) -> ResourceStateDocument {
    match state {
        ResourceState::Active => ResourceStateDocument::Active,
        ResourceState::InactiveByUser => ResourceStateDocument::InactiveByUser,
        ResourceState::InactiveByRestrictedPlan => ResourceStateDocument::InactiveByRestrictedPlan,
    }
}

fn state_from_document(state: ResourceStateDocument) -> ResourceState {
    match state {
        ResourceStateDocument::Active => ResourceState::Active,
        ResourceStateDocument::InactiveByUser => ResourceState::InactiveByUser,
        ResourceStateDocument::InactiveByRestrictedPlan => ResourceState::InactiveByRestrictedPlan,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct DistanceDocument {
    amount: f64,
    unit: DistanceUnitDocument,
}

impl From<Distance> for DistanceDocument {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

impl From<DistanceDocument> for Distance {
    fn from(value: DistanceDocument) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct GeoDistanceQueryDocument {
    lat: f64,
    lon: f64,
    distance: DistanceDocument,
}

impl From<GeoDistanceQuery> for GeoDistanceQueryDocument {
    fn from(value: GeoDistanceQuery) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

impl From<GeoDistanceQueryDocument> for GeoDistanceQuery {
    fn from(value: GeoDistanceQueryDocument) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum DistanceUnitDocument {
    Miles,
    Yards,
    Feet,
    Inches,
    Kilometers,
    Meters,
    Centimeters,
    Millimeters,
    NauticalMiles,
}

impl From<DistanceUnit> for DistanceUnitDocument {
    fn from(value: DistanceUnit) -> Self {
        match value {
            DistanceUnit::Miles => Self::Miles,
            DistanceUnit::Yards => Self::Yards,
            DistanceUnit::Feet => Self::Feet,
            DistanceUnit::Inches => Self::Inches,
            DistanceUnit::Kilometers => Self::Kilometers,
            DistanceUnit::Meters => Self::Meters,
            DistanceUnit::Centimeters => Self::Centimeters,
            DistanceUnit::Millimeters => Self::Millimeters,
            DistanceUnit::NauticalMiles => Self::NauticalMiles,
        }
    }
}

impl From<DistanceUnitDocument> for DistanceUnit {
    fn from(value: DistanceUnitDocument) -> Self {
        match value {
            DistanceUnitDocument::Miles => Self::Miles,
            DistanceUnitDocument::Yards => Self::Yards,
            DistanceUnitDocument::Feet => Self::Feet,
            DistanceUnitDocument::Inches => Self::Inches,
            DistanceUnitDocument::Kilometers => Self::Kilometers,
            DistanceUnitDocument::Meters => Self::Meters,
            DistanceUnitDocument::Centimeters => Self::Centimeters,
            DistanceUnitDocument::Millimeters => Self::Millimeters,
            DistanceUnitDocument::NauticalMiles => Self::NauticalMiles,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductSearchDocument {
    language: LanguageDocument,
    currency: CurrencyDocument,
    #[serde(rename = "productQuery")]
    product_query: Vec<TextQuery<1>>,
    #[serde(rename = "enhancedSearchDescription")]
    enhanced_search_description: Option<String>,
    #[serde(rename = "excludeProductId")]
    exclude_product_id_query: HashSet<ProductId>,
    #[serde(rename = "shopName")]
    shop_name_query: HashSet<ShopName>,
    #[serde(rename = "excludeShopName")]
    exclude_shop_name_query: HashSet<ShopName>,
    #[serde(rename = "sellerName")]
    seller_name_query: HashSet<ShopName>,
    #[serde(rename = "excludeSellerName")]
    exclude_seller_name_query: HashSet<ShopName>,
    #[serde(rename = "shopSlugId")]
    shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(rename = "excludeShopSlugId")]
    exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    #[serde(rename = "sellerSlugId")]
    seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(rename = "excludeSellerSlugId")]
    exclude_seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(rename = "shopType")]
    shop_type_query: HashSet<ShopTypeDocument>,
    #[serde(rename = "country")]
    country_query: HashSet<CountryCode>,
    #[serde(rename = "continent")]
    continent_query: HashSet<ContinentData>,
    #[serde(rename = "geoAddress")]
    geo_address_distance_query: Option<GeoDistanceQueryDocument>,
    #[serde(rename = "price")]
    price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "state")]
    state_query: HashSet<ProductStateDocument>,
    #[serde(rename = "lifecycle")]
    lifecycle_query: HashSet<ProductLifecycleDocument>,
    #[serde(rename = "created")]
    created_query: Option<TimeRangeDocument>,
    #[serde(rename = "updated")]
    updated_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionStart")]
    auction_start_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionEnd")]
    auction_end_query: Option<TimeRangeDocument>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LanguageDocument {
    De,
    En,
    Fr,
    Es,
    It,
    Zh,
    Pt,
    Pl,
    Tr,
    Nl,
    Cs,
    Ja,
    Ru,
    Ar,
}

impl From<Language> for LanguageDocument {
    fn from(value: Language) -> Self {
        match value {
            Language::De => Self::De,
            Language::En => Self::En,
            Language::Fr => Self::Fr,
            Language::Es => Self::Es,
            Language::It => Self::It,
            Language::Zh => Self::Zh,
            Language::Pt => Self::Pt,
            Language::Pl => Self::Pl,
            Language::Tr => Self::Tr,
            Language::Nl => Self::Nl,
            Language::Cs => Self::Cs,
            Language::Ja => Self::Ja,
            Language::Ru => Self::Ru,
            Language::Ar => Self::Ar,
        }
    }
}
impl From<LanguageDocument> for Language {
    fn from(value: LanguageDocument) -> Self {
        match value {
            LanguageDocument::De => Self::De,
            LanguageDocument::En => Self::En,
            LanguageDocument::Fr => Self::Fr,
            LanguageDocument::Es => Self::Es,
            LanguageDocument::It => Self::It,
            LanguageDocument::Zh => Self::Zh,
            LanguageDocument::Pt => Self::Pt,
            LanguageDocument::Pl => Self::Pl,
            LanguageDocument::Tr => Self::Tr,
            LanguageDocument::Nl => Self::Nl,
            LanguageDocument::Cs => Self::Cs,
            LanguageDocument::Ja => Self::Ja,
            LanguageDocument::Ru => Self::Ru,
            LanguageDocument::Ar => Self::Ar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CurrencyDocument {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
}

impl From<Currency> for CurrencyDocument {
    fn from(value: Currency) -> Self {
        match value {
            Currency::Eur => Self::Eur,
            Currency::Gbp => Self::Gbp,
            Currency::Usd => Self::Usd,
            Currency::Aud => Self::Aud,
            Currency::Cad => Self::Cad,
            Currency::Nzd => Self::Nzd,
            Currency::Cny => Self::Cny,
            Currency::Brl => Self::Brl,
            Currency::Pln => Self::Pln,
            Currency::Try => Self::Try,
            Currency::Jpy => Self::Jpy,
            Currency::Czk => Self::Czk,
            Currency::Rub => Self::Rub,
            Currency::Aed => Self::Aed,
            Currency::Sar => Self::Sar,
            Currency::Hkd => Self::Hkd,
            Currency::Sgd => Self::Sgd,
            Currency::Chf => Self::Chf,
        }
    }
}
impl From<CurrencyDocument> for Currency {
    fn from(value: CurrencyDocument) -> Self {
        match value {
            CurrencyDocument::Eur => Self::Eur,
            CurrencyDocument::Gbp => Self::Gbp,
            CurrencyDocument::Usd => Self::Usd,
            CurrencyDocument::Aud => Self::Aud,
            CurrencyDocument::Cad => Self::Cad,
            CurrencyDocument::Nzd => Self::Nzd,
            CurrencyDocument::Cny => Self::Cny,
            CurrencyDocument::Brl => Self::Brl,
            CurrencyDocument::Pln => Self::Pln,
            CurrencyDocument::Try => Self::Try,
            CurrencyDocument::Jpy => Self::Jpy,
            CurrencyDocument::Czk => Self::Czk,
            CurrencyDocument::Rub => Self::Rub,
            CurrencyDocument::Aed => Self::Aed,
            CurrencyDocument::Sar => Self::Sar,
            CurrencyDocument::Hkd => Self::Hkd,
            CurrencyDocument::Sgd => Self::Sgd,
            CurrencyDocument::Chf => Self::Chf,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateDocument {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductState> for ProductStateDocument {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}

impl From<ProductStateDocument> for ProductState {
    fn from(value: ProductStateDocument) -> Self {
        match value {
            ProductStateDocument::Listed => Self::Listed,
            ProductStateDocument::Available => Self::Available,
            ProductStateDocument::Reserved => Self::Reserved,
            ProductStateDocument::Sold => Self::Sold,
            ProductStateDocument::Removed => Self::Removed,
            ProductStateDocument::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductLifecycleDocument {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycle> for ProductLifecycleDocument {
    fn from(value: ProductLifecycle) -> Self {
        match value {
            ProductLifecycle::Active => Self::Active,
            ProductLifecycle::Deleted => Self::Deleted,
        }
    }
}

impl From<ProductLifecycleDocument> for ProductLifecycle {
    fn from(value: ProductLifecycleDocument) -> Self {
        match value {
            ProductLifecycleDocument::Active => Self::Active,
            ProductLifecycleDocument::Deleted => Self::Deleted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopTypeDocument {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl From<ShopType> for ShopTypeDocument {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
        }
    }
}

impl From<ShopTypeDocument> for ShopType {
    fn from(value: ShopTypeDocument) -> Self {
        match value {
            ShopTypeDocument::AuctionHouse => Self::AuctionHouse,
            ShopTypeDocument::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeDocument::CommercialDealer => Self::CommercialDealer,
            ShopTypeDocument::Marketplace => Self::Marketplace,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeDocument {
    min: Option<String>,
    max: Option<String>,
}

impl TryFrom<RangeQuery<OffsetDateTime>> for TimeRangeDocument {
    type Error = serde_json::Error;

    fn try_from(value: RangeQuery<OffsetDateTime>) -> Result<Self, Self::Error> {
        Ok(Self {
            min: value
                .min
                .map(|time| time.format(&Rfc3339))
                .transpose()
                .map_err(serde_json::Error::custom)?,
            max: value
                .max
                .map(|time| time.format(&Rfc3339))
                .transpose()
                .map_err(serde_json::Error::custom)?,
        })
    }
}

impl TryFrom<TimeRangeDocument> for RangeQuery<OffsetDateTime> {
    type Error = ();

    fn try_from(value: TimeRangeDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            min: value
                .min
                .map(|time| OffsetDateTime::parse(&time, &Rfc3339))
                .transpose()
                .map_err(|_| ())?,
            max: value
                .max
                .map(|time| OffsetDateTime::parse(&time, &Rfc3339))
                .transpose()
                .map_err(|_| ())?,
        })
    }
}

impl TryFrom<&ProductSearch> for ProductSearchDocument {
    type Error = serde_json::Error;

    fn try_from(search: &ProductSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: search.language.into(),
            currency: search.currency.into(),
            product_query: search.product_query.clone(),
            enhanced_search_description: search
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_id_query: search.exclude_product_id_query.iter().copied().collect(),
            shop_name_query: search.shop_name_query.iter().cloned().collect(),
            exclude_shop_name_query: search.exclude_shop_name_query.iter().cloned().collect(),
            seller_name_query: search.seller_name_query.iter().cloned().collect(),
            exclude_seller_name_query: search.exclude_seller_name_query.iter().cloned().collect(),
            shop_slug_id_query: search.shop_slug_id_query.iter().cloned().collect(),
            exclude_shop_slug_id_query: search.exclude_shop_slug_id_query.iter().cloned().collect(),
            seller_slug_id_query: search.seller_slug_id_query.iter().cloned().collect(),
            exclude_seller_slug_id_query: search
                .exclude_seller_slug_id_query
                .iter()
                .cloned()
                .collect(),
            shop_type_query: search
                .shop_type_query
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            country_query: search.country_query.iter().copied().collect(),
            continent_query: search
                .continent_query
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search.price_query.map(|range| range.map(u64::from)),
            state_query: search.state_query.iter().copied().map(Into::into).collect(),
            lifecycle_query: search
                .lifecycle_query
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            created_query: search.created_query.map(TryInto::try_into).transpose()?,
            updated_query: search.updated_query.map(TryInto::try_into).transpose()?,
            auction_start_query: search
                .auction_start_query
                .map(TryInto::try_into)
                .transpose()?,
            auction_end_query: search
                .auction_end_query
                .map(TryInto::try_into)
                .transpose()?,
        })
    }
}

impl TryFrom<ProductSearchDocument> for ProductSearch {
    type Error = ProductSearchDocumentMappingError;

    fn try_from(document: ProductSearchDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            language: document.language.into(),
            currency: document.currency.into(),
            product_query: document.product_query,
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            exclude_product_id_query: document.exclude_product_id_query.into(),
            shop_name_query: document.shop_name_query.into(),
            exclude_shop_name_query: document.exclude_shop_name_query.into(),
            seller_name_query: document.seller_name_query.into(),
            exclude_seller_name_query: document.exclude_seller_name_query.into(),
            shop_slug_id_query: document.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: document.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: document.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: document.exclude_seller_slug_id_query.into(),
            shop_type_query: document
                .shop_type_query
                .into_iter()
                .map(Into::into)
                .collect::<AnyOfQuery<_>>(),
            country_query: document.country_query.into(),
            continent_query: document
                .continent_query
                .into_iter()
                .map(Into::into)
                .collect::<AnyOfQuery<_>>(),
            geo_address_distance_query: document.geo_address_distance_query.map(Into::into),
            price_query: document
                .price_query
                .map(|range| range.map(MonetaryAmount::from)),
            state_query: document
                .state_query
                .into_iter()
                .map(Into::into)
                .collect::<AnyOfQuery<_>>(),
            lifecycle_query: document
                .lifecycle_query
                .into_iter()
                .map(Into::into)
                .collect::<AnyOfQuery<_>>(),
            created_query: document.created_query.map(parse_time_range).transpose()?,
            updated_query: document.updated_query.map(parse_time_range).transpose()?,
            auction_start_query: document
                .auction_start_query
                .map(parse_time_range)
                .transpose()?,
            auction_end_query: document
                .auction_end_query
                .map(parse_time_range)
                .transpose()?,
        })
    }
}

fn parse_time_range(
    value: TimeRangeDocument,
) -> Result<RangeQuery<OffsetDateTime>, ProductSearchDocumentMappingError> {
    value
        .try_into()
        .map_err(|_| ProductSearchDocumentMappingError::InvalidTimestamp)
}

fn product_search_to_value(search: &ProductSearch) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(ProductSearchDocument::try_from(search)?)
}

fn product_search_from_value(
    value: serde_json::Value,
) -> Result<ProductSearch, ProductSearchDocumentMappingError> {
    let Some(object) = value.as_object() else {
        return Err(ProductSearchDocumentMappingError::InvalidTimestamp);
    };
    if !PRODUCT_SEARCH_FIELDS
        .iter()
        .all(|field| object.contains_key(*field))
    {
        return Err(ProductSearchDocumentMappingError::InvalidTimestamp);
    }

    let document = serde_json::from_value::<ProductSearchDocument>(value)
        .map_err(|source| ProductSearchDocumentMappingError::Deserialize { source })?;
    document.try_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::query::range_query::RangeQuery;
    use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
    use localization::Language;
    use money::Currency;
    use search_filter_service::ports::SearchFilterProjection;
    use time::macros::datetime;

    fn projection(search: ProductSearch) -> SearchFilterProjection {
        SearchFilterProjection {
            view: SearchFilterView {
                search_filter_id: UserSearchFilterId::new(),
                user_id: UserId::new(),
                name: UserSearchFilterName::from("daily"),
                notifications: true,
                state: search_filter_core::ResourceState::Active,
                search,
                embedding: Some(vec![1.0]),
                created: datetime!(2026-01-01 00:00:00 UTC),
                updated: datetime!(2026-01-02 00:00:00 UTC),
                last_hybrid_search_matched: datetime!(2026-01-03 00:00:00 UTC),
            },
            source_version: 12,
        }
    }

    #[test]
    fn should_encode_legacy_lifecycle_document_in_screaming_snake_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let lifecycle = ProductLifecycleDocument::from(
            product_core::product_lifecycle::ProductLifecycle::Deleted,
        );

        assert_eq!(
            serde_json::json!("DELETED"),
            serde_json::to_value(lifecycle)?
        );
        assert_eq!(
            product_core::product_lifecycle::ProductLifecycle::Deleted,
            serde_json::from_value::<ProductLifecycleDocument>(serde_json::json!("DELETED"))?
                .into()
        );
        Ok(())
    }

    #[test]
    fn should_store_authoritative_search_version_and_original_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
            min: Some(MonetaryAmount::from(10_000_u64)),
            max: Some(MonetaryAmount::from(50_000_u64)),
        });
        let document = SearchFilterDocument::try_from(&projection(search))?;
        let value = serde_json::to_value(&document)?;

        assert_eq!(12, document.source_version);
        assert_eq!(
            Some(&serde_json::json!({ "gte": 10_000, "lte": 50_000 })),
            document
                .query
                .pointer("/bool/filter/1/range/priceByCurrency.usd")
        );
        assert!(value.get("compiledFxRateId").is_none());
        assert!(value.get("compiledFxGeneration").is_none());
        Ok(())
    }

    #[test]
    fn should_round_trip_geo_distance_query_with_legacy_document_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = projection(
            ProductSearch::new(Language::En, Currency::Usd).with_geo_address_distance_query(
                GeoDistanceQuery {
                    lat: 52.52,
                    lon: 13.405,
                    distance: Distance {
                        amount: 50.0,
                        unit: DistanceUnit::Kilometers,
                    },
                },
            ),
        );
        let document = SearchFilterDocument::try_from(&expected)?;
        let value = serde_json::to_value(&document)?;

        assert_eq!(
            Some(&serde_json::json!("KILOMETERS")),
            value.pointer("/search/geoAddress/distance/unit")
        );
        assert_eq!(expected.view, SearchFilterView::try_from(document)?);
        Ok(())
    }

    #[test]
    fn should_round_trip_search_filter_without_a_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = projection(ProductSearch::new(Language::En, Currency::Usd));
        let document = SearchFilterDocument::try_from(&expected)?;

        assert!(!document.query.to_string().contains("priceByCurrency"));
        assert_eq!(expected.view, SearchFilterView::try_from(document)?);
        Ok(())
    }
}
