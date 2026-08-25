use application::error::box_error;
use domain_primitives::event_id::EventId;
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::FxRateId;
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing::ProductListingPriceValuationBasis;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_state::ProductState;
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use shop_core::{seller_slug_id::SellerSlugId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use user_core::user_id::UserId;

use geo::{
    core::distance::{Distance, DistanceUnit, GeoDistanceQuery},
    data::continent_data::ContinentData,
};
use isocountry::CountryCode;
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ProductListingSearch,
};
use search_filter_core::{SearchFilter, SearchFilterProductListingMatch};
use search_filter_service::ports::{
    PersistedSearchFilter, PersistedSearchFilterMatch, SearchFilterIndexReadError,
    SearchFilterMatchView, SearchFilterProjection, SearchFilterReadError,
    SearchFilterRepositoryError, SearchFilterView,
};
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
use sqlx::FromRow;
use std::{collections::HashSet, error::Error, fmt};
use strum::IntoEnumIterator;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const FILTER_COLUMNS: &str = "user_search_filter_id, user_id, name, notifications, state, search, embedding, created, updated, version";
pub(crate) const MATCH_COLUMNS: &str = "user_id, user_search_filter_id, product_id, origin_event_id, price_valuation_basis, price_fx_rate_id, user_search_filter_name, enhanced_match_reason, feedback, created, updated";

#[derive(Debug)]
pub(crate) enum SearchFilterRowMappingError {
    NameTooLong,
    InvalidState,
    InvalidPriceMatchValuation,
}

impl fmt::Display for SearchFilterRowMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameTooLong => {
                formatter.write_str("persisted search filter name exceeds 255 characters")
            }
            Self::InvalidState => formatter.write_str("persisted search filter state is invalid"),
            Self::InvalidPriceMatchValuation => {
                formatter.write_str("persisted price match valuation is invalid")
            }
        }
    }
}

impl Error for SearchFilterRowMappingError {}

#[derive(Debug)]
pub(crate) enum ProductListingSearchJsonMappingError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    FormatTimestamp(time::error::Format),
    ParseTimestamp(time::error::Parse),
    EnhancedSearchDescription(EnhancedSearchDescriptionError),
}

impl fmt::Display for ProductListingSearchJsonMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(_) => {
                formatter.write_str("search filter product search JSON serialization failed")
            }
            Self::Deserialize(_) => {
                formatter.write_str("persisted search filter product search JSON is invalid")
            }
            Self::FormatTimestamp(_) => {
                formatter.write_str("search filter product search timestamp formatting failed")
            }
            Self::ParseTimestamp(_) => {
                formatter.write_str("search filter product search timestamp is invalid")
            }
            Self::EnhancedSearchDescription(_) => {
                formatter.write_str("persisted enhanced search description is invalid")
            }
        }
    }
}

impl Error for ProductListingSearchJsonMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(source) | Self::Deserialize(source) => Some(source),
            Self::FormatTimestamp(source) => Some(source),
            Self::ParseTimestamp(source) => Some(source),
            Self::EnhancedSearchDescription(source) => Some(source),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct FilterRow {
    pub user_search_filter_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub notifications: bool,
    pub state: String,
    pub search: serde_json::Value,
    pub embedding: Option<Vec<f32>>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub version: i64,
}
impl FilterRow {
    pub(crate) fn into_persisted(
        self,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let created = self.created;
        let updated = self.updated;
        let filter = SearchFilter::rehydrate(
            UserSearchFilterId::from(self.user_search_filter_id),
            UserId::from(self.user_id),
            name(self.name).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            self.notifications,
            state(&self.state).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            product_search_from_json(self.search).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            self.embedding,
        );
        Ok(PersistedSearchFilter {
            filter,
            created,
            updated,
            version: self.version,
        })
    }
    pub(crate) fn into_view(self) -> Result<SearchFilterView, SearchFilterReadError> {
        let created = self.created;
        let updated = self.updated;
        Ok(SearchFilterView {
            search_filter_id: UserSearchFilterId::from(self.user_search_filter_id),
            user_id: UserId::from(self.user_id),
            name: name(self.name).map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            notifications: self.notifications,
            state: state(&self.state).map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            search: product_search_from_json(self.search)
                .map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            embedding: self.embedding,
            created,
            updated,
        })
    }

    pub(crate) fn into_projection(
        self,
    ) -> Result<SearchFilterProjection, SearchFilterIndexReadError> {
        let source_version = self.version;
        let created = self.created;
        let updated = self.updated;
        let view = SearchFilterView {
            search_filter_id: UserSearchFilterId::from(self.user_search_filter_id),
            user_id: UserId::from(self.user_id),
            name: name(self.name).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            notifications: self.notifications,
            state: state(&self.state).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            search: product_search_from_json(self.search).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            embedding: self.embedding,
            created,
            updated,
        };
        Ok(SearchFilterProjection {
            view,
            source_version,
        })
    }
}
#[derive(Debug, FromRow)]
pub(crate) struct MatchRow {
    pub user_id: uuid::Uuid,
    pub user_search_filter_id: uuid::Uuid,
    pub product_id: uuid::Uuid,
    pub origin_event_id: uuid::Uuid,
    pub price_valuation_basis: Option<String>,
    pub price_fx_rate_id: Option<uuid::Uuid>,
    pub user_search_filter_name: Option<String>,
    pub enhanced_match_reason: Option<String>,
    pub feedback: Option<bool>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
impl TryFrom<MatchRow> for PersistedSearchFilterMatch {
    type Error = SearchFilterRowMappingError;
    fn try_from(row: MatchRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_match: SearchFilterProductListingMatch {
                user_id: UserId::from(row.user_id),
                user_search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
                user_search_filter_name: row.user_search_filter_name.map(name).transpose()?,
                product_id: ProductListingId::from(row.product_id),
                origin_event_id: EventId::from(row.origin_event_id),
                price_match_valuation: price_match_valuation(
                    row.price_valuation_basis.as_deref(),
                    row.price_fx_rate_id,
                )?,
                enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
                feedback: row.feedback,
            },
            created: row.created,
            updated: row.updated,
        })
    }
}
impl TryFrom<MatchRow> for SearchFilterMatchView {
    type Error = SearchFilterRowMappingError;
    fn try_from(row: MatchRow) -> Result<Self, Self::Error> {
        price_match_valuation(row.price_valuation_basis.as_deref(), row.price_fx_rate_id)?;
        Ok(Self {
            user_id: UserId::from(row.user_id),
            search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
            search_filter_name: row.user_search_filter_name.map(name).transpose()?,
            product_id: ProductListingId::from(row.product_id),
            origin_event_id: EventId::from(row.origin_event_id),
            enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            feedback: row.feedback,
            created: row.created,
            updated: row.updated,
        })
    }
}

pub(crate) fn user_search_filter_uuid(id: UserSearchFilterId) -> Result<uuid::Uuid, uuid::Error> {
    uuid::Uuid::parse_str(&id.to_string())
}
fn price_match_valuation(
    basis: Option<&str>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<search_filter_core::PriceMatchValuation>, SearchFilterRowMappingError> {
    match (basis, fx_rate_id) {
        (None, None) => Ok(None),
        (Some(basis), Some(fx_rate_id)) => ProductListingPriceValuationBasis::iter()
            .find(|candidate| candidate.as_str() == basis)
            .map(|basis| search_filter_core::PriceMatchValuation {
                basis,
                fx_rate_id: FxRateId::from(fx_rate_id),
            })
            .ok_or(SearchFilterRowMappingError::InvalidPriceMatchValuation)
            .map(Some),
        _ => Err(SearchFilterRowMappingError::InvalidPriceMatchValuation),
    }
}

pub(crate) fn state(value: &str) -> Result<SearchFilterState, SearchFilterRowMappingError> {
    SearchFilterState::from_code(value).ok_or(SearchFilterRowMappingError::InvalidState)
}
pub(crate) fn name(v: String) -> Result<UserSearchFilterName, SearchFilterRowMappingError> {
    if v.len() > 255 {
        Err(SearchFilterRowMappingError::NameTooLong)
    } else {
        Ok(v.into())
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct DistanceJson {
    amount: f64,
    #[serde(with = "distance_unit")]
    unit: DistanceUnit,
}

impl From<Distance> for DistanceJson {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
        }
    }
}

impl From<DistanceJson> for Distance {
    fn from(value: DistanceJson) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct GeoDistanceQueryJson {
    lat: f64,
    lon: f64,
    distance: DistanceJson,
}

impl From<GeoDistanceQuery> for GeoDistanceQueryJson {
    fn from(value: GeoDistanceQuery) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

impl From<GeoDistanceQueryJson> for GeoDistanceQuery {
    fn from(value: GeoDistanceQueryJson) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingSearchJson {
    #[serde(with = "language")]
    language: Language,
    #[serde(with = "currency")]
    currency: Currency,
    product_listing_query: Vec<domain_primitives::query::text_query::TextQuery<1>>,
    enhanced_search_description: Option<String>,
    exclude_product_listing_id_query: HashSet<ProductListingId>,
    shop_name_query: HashSet<ShopName>,
    exclude_shop_name_query: HashSet<ShopName>,
    seller_name_query: HashSet<ShopName>,
    exclude_seller_name_query: HashSet<ShopName>,
    shop_slug_id_query: HashSet<ShopSlugId>,
    exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    seller_slug_id_query: HashSet<SellerSlugId>,
    exclude_seller_slug_id_query: HashSet<SellerSlugId>,
    #[serde(with = "shop_type")]
    shop_type_query: HashSet<ShopType>,
    country_query: HashSet<CountryCode>,
    continent_query: HashSet<ContinentData>,
    geo_address_distance_query: Option<GeoDistanceQueryJson>,
    price_query: Option<RangeQuery<u64>>,
    #[serde(with = "product_state")]
    state_query: HashSet<ProductState>,
    #[serde(with = "product_lifecycle")]
    lifecycle_query: HashSet<ProductLifecycle>,
    created_query: Option<TimeRangeJson>,
    updated_query: Option<TimeRangeJson>,
    auction_start_query: Option<TimeRangeJson>,
    auction_end_query: Option<TimeRangeJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeJson {
    min: Option<String>,
    max: Option<String>,
}
impl TryFrom<RangeQuery<OffsetDateTime>> for TimeRangeJson {
    type Error = ProductListingSearchJsonMappingError;
    fn try_from(v: RangeQuery<OffsetDateTime>) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::FormatTimestamp)?,
            max: v
                .max
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::FormatTimestamp)?,
        })
    }
}
impl TryFrom<TimeRangeJson> for RangeQuery<OffsetDateTime> {
    type Error = ProductListingSearchJsonMappingError;
    fn try_from(v: TimeRangeJson) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::ParseTimestamp)?,
            max: v
                .max
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::ParseTimestamp)?,
        })
    }
}
impl TryFrom<&ProductListingSearch> for ProductListingSearchJson {
    type Error = ProductListingSearchJsonMappingError;

    fn try_from(v: &ProductListingSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: v.language,
            currency: v.currency,
            product_listing_query: v.product_listing_query.clone(),
            enhanced_search_description: v
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_listing_id_query: v
                .exclude_product_listing_id_query
                .iter()
                .copied()
                .collect(),
            shop_name_query: v.shop_name_query.iter().cloned().collect(),
            exclude_shop_name_query: v.exclude_shop_name_query.iter().cloned().collect(),
            seller_name_query: v.seller_name_query.iter().cloned().collect(),
            exclude_seller_name_query: v.exclude_seller_name_query.iter().cloned().collect(),
            shop_slug_id_query: v.shop_slug_id_query.iter().cloned().collect(),
            exclude_shop_slug_id_query: v.exclude_shop_slug_id_query.iter().cloned().collect(),
            seller_slug_id_query: v.seller_slug_id_query.iter().cloned().collect(),
            exclude_seller_slug_id_query: v.exclude_seller_slug_id_query.iter().cloned().collect(),
            shop_type_query: v.shop_type_query.iter().copied().collect(),
            country_query: v.country_query.iter().copied().collect(),
            continent_query: v.continent_query.iter().copied().map(Into::into).collect(),
            geo_address_distance_query: v.geo_address_distance_query.map(Into::into),
            price_query: v.price_query.map(|v| v.map(u64::from)),
            state_query: v.state_query.iter().copied().collect(),
            lifecycle_query: v.lifecycle_query.iter().copied().collect(),
            created_query: v.created_query.map(TimeRangeJson::try_from).transpose()?,
            updated_query: v.updated_query.map(TimeRangeJson::try_from).transpose()?,
            auction_start_query: v
                .auction_start_query
                .map(TimeRangeJson::try_from)
                .transpose()?,
            auction_end_query: v
                .auction_end_query
                .map(TimeRangeJson::try_from)
                .transpose()?,
        })
    }
}
pub(crate) fn product_search_from_json(
    v: serde_json::Value,
) -> Result<ProductListingSearch, ProductListingSearchJsonMappingError> {
    let j: ProductListingSearchJson =
        serde_json::from_value(v).map_err(ProductListingSearchJsonMappingError::Deserialize)?;
    Ok(ProductListingSearch {
        language: j.language,
        currency: j.currency,
        product_listing_query: j.product_listing_query,
        enhanced_search_description: j
            .enhanced_search_description
            .map(EnhancedSearchDescription::try_from)
            .transpose()
            .map_err(ProductListingSearchJsonMappingError::EnhancedSearchDescription)?,
        exclude_product_listing_id_query: j.exclude_product_listing_id_query.into(),
        shop_name_query: j.shop_name_query.into(),
        exclude_shop_name_query: j.exclude_shop_name_query.into(),
        seller_name_query: j.seller_name_query.into(),
        exclude_seller_name_query: j.exclude_seller_name_query.into(),
        shop_slug_id_query: j.shop_slug_id_query.into(),
        exclude_shop_slug_id_query: j.exclude_shop_slug_id_query.into(),
        seller_slug_id_query: j.seller_slug_id_query.into(),
        exclude_seller_slug_id_query: j.exclude_seller_slug_id_query.into(),
        shop_type_query: j.shop_type_query.into(),
        country_query: j.country_query.into(),
        continent_query: j
            .continent_query
            .into_iter()
            .map(Into::into)
            .collect::<AnyOfQuery<_>>(),
        geo_address_distance_query: j.geo_address_distance_query.map(Into::into),
        price_query: j.price_query.map(|v| v.map(Into::into)),
        state_query: j.state_query.into(),
        lifecycle_query: j.lifecycle_query.into(),
        created_query: j.created_query.map(TryInto::try_into).transpose()?,
        updated_query: j.updated_query.map(TryInto::try_into).transpose()?,
        auction_start_query: j.auction_start_query.map(TryInto::try_into).transpose()?,
        auction_end_query: j.auction_end_query.map(TryInto::try_into).transpose()?,
    })
}
pub(crate) fn product_search_to_json(
    v: &ProductListingSearch,
) -> Result<serde_json::Value, ProductListingSearchJsonMappingError> {
    serde_json::to_value(ProductListingSearchJson::try_from(v)?)
        .map_err(ProductListingSearchJsonMappingError::Serialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;

    #[test]
    fn should_round_trip_geo_distance_query_with_legacy_json_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductListingSearch::new(Language::De, Currency::Usd)
            .with_geo_address_distance_query(GeoDistanceQuery {
                lat: 52.52,
                lon: 13.405,
                distance: Distance {
                    amount: 50.0,
                    unit: DistanceUnit::Kilometers,
                },
            });

        let json = product_search_to_json(&search)?;

        assert_eq!(
            Some(&serde_json::json!("KILOMETERS")),
            json.pointer("/geo_address_distance_query/distance/unit")
        );
        assert_eq!(search, product_search_from_json(json)?);
        Ok(())
    }

    #[test]
    fn should_round_trip_full_product_search_json() {
        let search = ProductListingSearch::new(Language::De, Currency::Usd)
            .with_product_listing_query(match "vase".try_into() {
                Ok(v) => v,
                Err(e) => panic!("bad test value: {e}"),
            });
        let json = match product_search_to_json(&search) {
            Ok(v) => v,
            Err(_) => panic!("serialize"),
        };
        let decoded = match product_search_from_json(json) {
            Ok(search) => search,
            Err(error) => panic!("failed to deserialize product search: {error}"),
        };
        assert_eq!(search, decoded);
    }

    #[test]
    fn should_preserve_legacy_product_search_leaf_codes() -> Result<(), Box<dyn Error>> {
        let mut persisted =
            product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
        persisted["shop_type_query"] = serde_json::json!(["COMMERCIAL_DEALER"]);
        persisted["state_query"] = serde_json::json!(["AVAILABLE"]);
        persisted["lifecycle_query"] = serde_json::json!(["ACTIVE"]);
        persisted["geo_address_distance_query"] = serde_json::json!({
            "lat": 52.52,
            "lon": 13.405,
            "distance": {"amount": 50.0, "unit": "KILOMETERS"}
        });

        assert_eq!(Some(&serde_json::json!("en")), persisted.get("language"));
        assert_eq!(Some(&serde_json::json!("EUR")), persisted.get("currency"));
        assert_eq!(
            Some(&serde_json::json!("COMMERCIAL_DEALER")),
            persisted.pointer("/shop_type_query/0")
        );
        assert_eq!(
            Some(&serde_json::json!("AVAILABLE")),
            persisted.pointer("/state_query/0")
        );
        assert_eq!(
            Some(&serde_json::json!("ACTIVE")),
            persisted.pointer("/lifecycle_query/0")
        );
        assert_eq!(
            Some(&serde_json::json!("KILOMETERS")),
            persisted.pointer("/geo_address_distance_query/distance/unit")
        );

        let decoded = product_search_from_json(persisted)?;
        assert_eq!(Language::En, decoded.language);
        assert_eq!(Currency::Eur, decoded.currency);
        assert!(
            decoded
                .shop_type_query
                .contains(&ShopType::CommercialDealer)
        );
        assert!(decoded.state_query.contains(&ProductState::Available));
        assert!(decoded.lifecycle_query.contains(&ProductLifecycle::Active));
        assert_eq!(
            Some(DistanceUnit::Kilometers),
            decoded
                .geo_address_distance_query
                .map(|query| query.distance.unit)
        );
        Ok(())
    }
    #[test]
    fn should_serialize_every_product_search_field() {
        let json =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(value) => value,
                Err(_) => panic!("failed to serialize product search"),
            };
        let object = match json.as_object() {
            Some(value) => value,
            None => panic!("product search JSON must be an object"),
        };

        assert_eq!(24, object.len());
        assert!(object.contains_key("auction_end_query"));
        assert!(object.contains_key("exclude_seller_slug_id_query"));
        assert!(object.contains_key("geo_address_distance_query"));
    }

    #[test]
    fn should_preserve_incomplete_product_search_json_source() {
        let error = match product_search_from_json(serde_json::json!({})) {
            Ok(_) => panic!("incomplete product search JSON must fail"),
            Err(error) => error,
        };

        assert!(std::error::Error::source(&error).is_some());
        assert!(matches!(
            error,
            ProductListingSearchJsonMappingError::Deserialize(_)
        ));
    }

    #[test]
    fn should_parse_each_canonical_state() {
        for expected in SearchFilterState::iter() {
            assert!(matches!(state(expected.as_str()), Ok(actual) if actual == expected));
        }
    }

    #[test]
    fn should_parse_each_canonical_price_match_valuation_basis() {
        let fx_rate_id = uuid::Uuid::nil();

        for expected in ProductListingPriceValuationBasis::iter() {
            let valuation = price_match_valuation(Some(expected.as_str()), Some(fx_rate_id));

            assert!(matches!(
                valuation,
                Ok(Some(actual)) if actual.basis == expected && actual.fx_rate_id == FxRateId::from(fx_rate_id)
            ));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_price_match_valuation_bases() {
        let fx_rate_id = uuid::Uuid::nil();

        for basis in ["bad", "current"] {
            assert!(matches!(
                price_match_valuation(Some(basis), Some(fx_rate_id)),
                Err(SearchFilterRowMappingError::InvalidPriceMatchValuation)
            ));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_states() {
        for value in ["bad", "active"] {
            assert!(matches!(
                state(value),
                Err(SearchFilterRowMappingError::InvalidState)
            ));
        }
    }

    #[test]
    fn should_preserve_invalid_filter_row_mapping_source() {
        let search =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(search) => search,
                Err(error) => panic!("failed to create product search JSON: {error}"),
            };
        let error = match (FilterRow {
            user_search_filter_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            name: "x".repeat(256),
            notifications: true,
            state: "ACTIVE".to_owned(),
            search,
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        })
        .into_persisted()
        {
            Ok(_) => panic!("overlong persisted filter name must fail"),
            Err(error) => error,
        };

        let SearchFilterRepositoryError::InvalidPersistedState { source } = error else {
            panic!("expected invalid persisted search-filter state");
        };
        assert!(matches!(
            source.downcast_ref::<SearchFilterRowMappingError>(),
            Some(SearchFilterRowMappingError::NameTooLong)
        ));
    }

    #[test]
    fn should_reject_unknown_product_search_field() {
        let mut json =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(value) => value,
                Err(_) => panic!("failed to serialize product search"),
            };
        let object = match json.as_object_mut() {
            Some(value) => value,
            None => panic!("product search JSON must be an object"),
        };
        object.insert("unexpected".into(), serde_json::Value::Null);

        assert!(product_search_from_json(json).is_err());
    }
}
