use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;

use geo::{
    core::distance::{Distance, DistanceUnit, GeoDistanceQuery},
    data::continent_data::ContinentData,
};
use isocountry::CountryCode;
use localization::Language;
use money::{Currency, MonetaryAmount};
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ProductListingSearch,
};
use product_listing_core::product_state::ProductState;
use product_listing_opensearch::build_percolator_query;
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_service::ports::{SearchFilterProjection, SearchFilterView};
use serde::ser::Error as _;
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
use shop_core::{seller_slug_id::SellerSlugId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use std::collections::HashSet;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use user_core::user_id::UserId;

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

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: serde::Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}

fn serialize_set_code<T, S>(
    values: &HashSet<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + std::hash::Hash,
    S: serde::Serializer,
{
    serializer.collect_seq(values.iter().map(|value| code(*value)))
}

fn deserialize_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<HashSet<T>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: serde::Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| {
            parse(&value)
                .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
        })
        .collect()
}

mod search_filter_state {
    use super::*;

    pub(crate) fn serialize<S>(value: &SearchFilterState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, SearchFilterState::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<SearchFilterState, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, SearchFilterState::from_code)
    }
}

mod distance_unit {
    use super::*;

    pub(crate) fn serialize<S>(value: &DistanceUnit, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, DistanceUnit::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<DistanceUnit, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, DistanceUnit::from_code)
    }
}

mod language {
    use super::*;

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Language::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Language::from_code)
    }
}

mod currency {
    use super::*;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code)
    }
}

mod shop_type {
    use super::*;

    pub(crate) fn serialize<S>(values: &HashSet<ShopType>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ShopType::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<ShopType>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ShopType::from_code)
    }
}

mod product_state {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ProductState>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ProductState::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<ProductState>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ProductState::from_code)
    }
}

mod product_lifecycle {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ProductLifecycle>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ProductLifecycle::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashSet<ProductLifecycle>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ProductLifecycle::from_code)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    #[serde(with = "search_filter_state")]
    pub state: SearchFilterState,
    pub source_version: i64,
    pub search: serde_json::Value,
    pub query: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// Decode failure for the complete product-search payload stored in a search document.
#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchDocumentMappingError {
    #[error("OpenSearch document has malformed product search JSON")]
    Deserialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("OpenSearch document has an invalid product search timestamp")]
    InvalidTimestamp,
    #[error("OpenSearch document has an invalid enhanced search description")]
    InvalidEnhancedSearchDescription {
        #[source]
        source: EnhancedSearchDescriptionError,
    },
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
            state: view.state,
            source_version: projection.source_version,
            search: product_search_to_value(&view.search)?,
            query: build_percolator_query(&view.search)?,
            embedding: view.embedding.clone(),
            created: view.created,
            updated: view.updated,
        })
    }
}

impl TryFrom<SearchFilterDocument> for SearchFilterView {
    type Error = ProductListingSearchDocumentMappingError;

    fn try_from(document: SearchFilterDocument) -> Result<Self, Self::Error> {
        Ok(SearchFilterView {
            search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            notifications: document.notifications,
            state: document.state,
            search: product_search_from_value(document.search)?,
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct DistanceDocument {
    amount: f64,
    #[serde(with = "distance_unit")]
    unit: DistanceUnit,
}

impl From<Distance> for DistanceDocument {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
        }
    }
}

impl From<DistanceDocument> for Distance {
    fn from(value: DistanceDocument) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingSearchDocument {
    #[serde(with = "language")]
    language: Language,
    #[serde(with = "currency")]
    currency: Currency,
    #[serde(rename = "productQuery")]
    product_listing_query: Vec<TextQuery<1>>,
    #[serde(rename = "enhancedSearchDescription")]
    enhanced_search_description: Option<String>,
    #[serde(rename = "excludeProductId")]
    exclude_product_listing_id_query: HashSet<ProductListingId>,
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
    #[serde(rename = "shopType", with = "shop_type")]
    shop_type_query: HashSet<ShopType>,
    #[serde(rename = "country")]
    country_query: HashSet<CountryCode>,
    #[serde(rename = "continent")]
    continent_query: HashSet<ContinentData>,
    #[serde(rename = "geoAddress")]
    geo_address_distance_query: Option<GeoDistanceQueryDocument>,
    #[serde(rename = "price")]
    price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "state", with = "product_state")]
    state_query: HashSet<ProductState>,
    #[serde(rename = "lifecycle", with = "product_lifecycle")]
    lifecycle_query: HashSet<ProductLifecycle>,
    #[serde(rename = "created")]
    created_query: Option<TimeRangeDocument>,
    #[serde(rename = "updated")]
    updated_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionStart")]
    auction_start_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionEnd")]
    auction_end_query: Option<TimeRangeDocument>,
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

impl TryFrom<&ProductListingSearch> for ProductListingSearchDocument {
    type Error = serde_json::Error;

    fn try_from(search: &ProductListingSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: search.language,
            currency: search.currency,
            product_listing_query: search.product_listing_query.clone(),
            enhanced_search_description: search
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_listing_id_query: search
                .exclude_product_listing_id_query
                .iter()
                .copied()
                .collect(),
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
            shop_type_query: search.shop_type_query.iter().copied().collect(),
            country_query: search.country_query.iter().copied().collect(),
            continent_query: search
                .continent_query
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            geo_address_distance_query: search.geo_address_distance_query.map(Into::into),
            price_query: search.price_query.map(|range| range.map(u64::from)),
            state_query: search.state_query.iter().copied().collect(),
            lifecycle_query: search.lifecycle_query.iter().copied().collect(),
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

impl TryFrom<ProductListingSearchDocument> for ProductListingSearch {
    type Error = ProductListingSearchDocumentMappingError;

    fn try_from(document: ProductListingSearchDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            language: document.language,
            currency: document.currency,
            product_listing_query: document.product_listing_query,
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|source| {
                    ProductListingSearchDocumentMappingError::InvalidEnhancedSearchDescription {
                        source,
                    }
                })?,
            exclude_product_listing_id_query: document.exclude_product_listing_id_query.into(),
            shop_name_query: document.shop_name_query.into(),
            exclude_shop_name_query: document.exclude_shop_name_query.into(),
            seller_name_query: document.seller_name_query.into(),
            exclude_seller_name_query: document.exclude_seller_name_query.into(),
            shop_slug_id_query: document.shop_slug_id_query.into(),
            exclude_shop_slug_id_query: document.exclude_shop_slug_id_query.into(),
            seller_slug_id_query: document.seller_slug_id_query.into(),
            exclude_seller_slug_id_query: document.exclude_seller_slug_id_query.into(),
            shop_type_query: document.shop_type_query.into(),
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
            state_query: document.state_query.into(),
            lifecycle_query: document.lifecycle_query.into(),
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
) -> Result<RangeQuery<OffsetDateTime>, ProductListingSearchDocumentMappingError> {
    value
        .try_into()
        .map_err(|_| ProductListingSearchDocumentMappingError::InvalidTimestamp)
}

fn product_search_to_value(
    search: &ProductListingSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(ProductListingSearchDocument::try_from(search)?)
}

fn product_search_from_value(
    value: serde_json::Value,
) -> Result<ProductListingSearch, ProductListingSearchDocumentMappingError> {
    let Some(object) = value.as_object() else {
        return Err(ProductListingSearchDocumentMappingError::InvalidTimestamp);
    };
    if !PRODUCT_SEARCH_FIELDS
        .iter()
        .all(|field| object.contains_key(*field))
    {
        return Err(ProductListingSearchDocumentMappingError::InvalidTimestamp);
    }

    let document = serde_json::from_value::<ProductListingSearchDocument>(value)
        .map_err(|source| ProductListingSearchDocumentMappingError::Deserialize { source })?;
    document.try_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::query::range_query::RangeQuery;
    use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
    use localization::Language;
    use money::Currency;
    use search_filter_service::ports::SearchFilterProjection;
    use time::macros::datetime;

    fn projection(search: ProductListingSearch) -> SearchFilterProjection {
        SearchFilterProjection {
            view: SearchFilterView {
                search_filter_id: UserSearchFilterId::new(),
                user_id: UserId::new(),
                name: UserSearchFilterName::from("daily"),
                notifications: true,
                state: search_filter_core::search_filter_state::SearchFilterState::Active,
                search,
                embedding: Some(vec![1.0]),
                created: datetime!(2026-01-01 00:00:00 UTC),
                updated: datetime!(2026-01-02 00:00:00 UTC),
            },
            source_version: 12,
        }
    }

    #[test]
    fn should_store_authoritative_search_version_and_original_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let search =
            ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
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
            ProductListingSearch::new(Language::En, Currency::Usd).with_geo_address_distance_query(
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
        let expected = projection(
            ProductListingSearch::new(Language::En, Currency::Usd)
                .with_shop_type_query(
                    std::collections::HashSet::from([ShopType::CommercialDealer]).into(),
                )
                .with_state_query(std::collections::HashSet::from([ProductState::Available]).into())
                .with_lifecycle_query(
                    std::collections::HashSet::from([ProductLifecycle::Active]).into(),
                ),
        );
        let document = SearchFilterDocument::try_from(&expected)?;
        let value = serde_json::to_value(&document)?;

        assert!(!document.query.to_string().contains("priceByCurrency"));
        assert_eq!(
            Some(&serde_json::json!("en")),
            value.pointer("/search/language")
        );
        assert_eq!(
            Some(&serde_json::json!("USD")),
            value.pointer("/search/currency")
        );
        assert_eq!(
            Some(&serde_json::json!("COMMERCIAL_DEALER")),
            value.pointer("/search/shopType/0")
        );
        assert_eq!(
            Some(&serde_json::json!("AVAILABLE")),
            value.pointer("/search/state/0")
        );
        assert_eq!(
            Some(&serde_json::json!("ACTIVE")),
            value.pointer("/search/lifecycle/0")
        );
        assert_eq!(expected.view, SearchFilterView::try_from(document)?);
        Ok(())
    }
}
