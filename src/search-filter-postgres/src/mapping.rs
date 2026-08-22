use application::error::box_error;
use domain_primitives::event_id::EventId;
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::FxRateId;
use product_core::product_id::ProductId;
use product_core::product_lifecycle::ProductLifecycle;
use product_core::product_state::ProductState;
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
use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use search_filter_core::{SearchFilter, SearchFilterProductMatch};
use search_filter_service::ports::{
    PersistedSearchFilter, PersistedSearchFilterMatch, SearchFilterIndexReadError,
    SearchFilterMatchView, SearchFilterProjection, SearchFilterReadError,
    SearchFilterRepositoryError, SearchFilterView,
};
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
use sqlx::FromRow;
use std::{collections::HashSet, error::Error, fmt};
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
pub(crate) enum ProductSearchJsonMappingError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    FormatTimestamp(time::error::Format),
    ParseTimestamp(time::error::Parse),
}

impl fmt::Display for ProductSearchJsonMappingError {
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
                formatter.write_str("persisted search filter product search timestamp is invalid")
            }
        }
    }
}

impl Error for ProductSearchJsonMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(source) | Self::Deserialize(source) => Some(source),
            Self::FormatTimestamp(source) => Some(source),
            Self::ParseTimestamp(source) => Some(source),
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
            product_match: SearchFilterProductMatch {
                user_id: UserId::from(row.user_id),
                user_search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
                user_search_filter_name: row.user_search_filter_name.map(name).transpose()?,
                product_id: ProductId::from(row.product_id),
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
            product_id: ProductId::from(row.product_id),
            origin_event_id: EventId::from(row.origin_event_id),
            enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            feedback: row.feedback,
            created: row.created,
            updated: row.updated,
        })
    }
}

pub(crate) fn format_state(value: SearchFilterState) -> &'static str {
    match value {
        SearchFilterState::Active => "ACTIVE",
        SearchFilterState::InactiveByUser => "INACTIVE_BY_USER",
        SearchFilterState::InactiveByRestrictedPlan => "INACTIVE_BY_RESTRICTED_PLAN",
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
        (Some(basis), Some(fx_rate_id)) => {
            product_core::product::ProductPriceValuationBasis::from_db_str(basis)
                .map(|basis| search_filter_core::PriceMatchValuation {
                    basis,
                    fx_rate_id: FxRateId::from(fx_rate_id),
                })
                .ok_or(SearchFilterRowMappingError::InvalidPriceMatchValuation)
                .map(Some)
        }
        _ => Err(SearchFilterRowMappingError::InvalidPriceMatchValuation),
    }
}

pub(crate) fn state(v: &str) -> Result<SearchFilterState, SearchFilterRowMappingError> {
    match v {
        "ACTIVE" => Ok(SearchFilterState::Active),
        "INACTIVE_BY_USER" => Ok(SearchFilterState::InactiveByUser),
        "INACTIVE_BY_RESTRICTED_PLAN" => Ok(SearchFilterState::InactiveByRestrictedPlan),
        _ => Err(SearchFilterRowMappingError::InvalidState),
    }
}
pub(crate) fn name(v: String) -> Result<UserSearchFilterName, SearchFilterRowMappingError> {
    if v.len() > 255 {
        Err(SearchFilterRowMappingError::NameTooLong)
    } else {
        Ok(v.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct DistanceJson {
    amount: f64,
    unit: DistanceUnitJson,
}

impl From<Distance> for DistanceJson {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

impl From<DistanceJson> for Distance {
    fn from(value: DistanceJson) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum DistanceUnitJson {
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

impl From<DistanceUnit> for DistanceUnitJson {
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

impl From<DistanceUnitJson> for DistanceUnit {
    fn from(value: DistanceUnitJson) -> Self {
        match value {
            DistanceUnitJson::Miles => Self::Miles,
            DistanceUnitJson::Yards => Self::Yards,
            DistanceUnitJson::Feet => Self::Feet,
            DistanceUnitJson::Inches => Self::Inches,
            DistanceUnitJson::Kilometers => Self::Kilometers,
            DistanceUnitJson::Meters => Self::Meters,
            DistanceUnitJson::Centimeters => Self::Centimeters,
            DistanceUnitJson::Millimeters => Self::Millimeters,
            DistanceUnitJson::NauticalMiles => Self::NauticalMiles,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductSearchJson {
    language: LanguageJson,
    currency: CurrencyJson,
    product_query: Vec<domain_primitives::query::text_query::TextQuery<1>>,
    enhanced_search_description: Option<String>,
    exclude_product_id_query: HashSet<ProductId>,
    shop_name_query: HashSet<ShopName>,
    exclude_shop_name_query: HashSet<ShopName>,
    seller_name_query: HashSet<ShopName>,
    exclude_seller_name_query: HashSet<ShopName>,
    shop_slug_id_query: HashSet<ShopSlugId>,
    exclude_shop_slug_id_query: HashSet<ShopSlugId>,
    seller_slug_id_query: HashSet<SellerSlugId>,
    exclude_seller_slug_id_query: HashSet<SellerSlugId>,
    shop_type_query: HashSet<ShopTypeJson>,
    country_query: HashSet<CountryCode>,
    continent_query: HashSet<ContinentData>,
    geo_address_distance_query: Option<GeoDistanceQueryJson>,
    price_query: Option<RangeQuery<u64>>,
    state_query: HashSet<ProductStateJson>,
    lifecycle_query: HashSet<ProductLifecycleJson>,
    created_query: Option<TimeRangeJson>,
    updated_query: Option<TimeRangeJson>,
    auction_start_query: Option<TimeRangeJson>,
    auction_end_query: Option<TimeRangeJson>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LanguageJson {
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

impl From<Language> for LanguageJson {
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

impl From<LanguageJson> for Language {
    fn from(value: LanguageJson) -> Self {
        match value {
            LanguageJson::De => Self::De,
            LanguageJson::En => Self::En,
            LanguageJson::Fr => Self::Fr,
            LanguageJson::Es => Self::Es,
            LanguageJson::It => Self::It,
            LanguageJson::Zh => Self::Zh,
            LanguageJson::Pt => Self::Pt,
            LanguageJson::Pl => Self::Pl,
            LanguageJson::Tr => Self::Tr,
            LanguageJson::Nl => Self::Nl,
            LanguageJson::Cs => Self::Cs,
            LanguageJson::Ja => Self::Ja,
            LanguageJson::Ru => Self::Ru,
            LanguageJson::Ar => Self::Ar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CurrencyJson {
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

impl From<Currency> for CurrencyJson {
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

impl From<CurrencyJson> for Currency {
    fn from(value: CurrencyJson) -> Self {
        match value {
            CurrencyJson::Eur => Self::Eur,
            CurrencyJson::Gbp => Self::Gbp,
            CurrencyJson::Usd => Self::Usd,
            CurrencyJson::Aud => Self::Aud,
            CurrencyJson::Cad => Self::Cad,
            CurrencyJson::Nzd => Self::Nzd,
            CurrencyJson::Cny => Self::Cny,
            CurrencyJson::Brl => Self::Brl,
            CurrencyJson::Pln => Self::Pln,
            CurrencyJson::Try => Self::Try,
            CurrencyJson::Jpy => Self::Jpy,
            CurrencyJson::Czk => Self::Czk,
            CurrencyJson::Rub => Self::Rub,
            CurrencyJson::Aed => Self::Aed,
            CurrencyJson::Sar => Self::Sar,
            CurrencyJson::Hkd => Self::Hkd,
            CurrencyJson::Sgd => Self::Sgd,
            CurrencyJson::Chf => Self::Chf,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopTypeJson {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}
impl From<ShopType> for ShopTypeJson {
    fn from(v: ShopType) -> Self {
        match v {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
        }
    }
}
impl From<ShopTypeJson> for ShopType {
    fn from(v: ShopTypeJson) -> Self {
        match v {
            ShopTypeJson::AuctionHouse => Self::AuctionHouse,
            ShopTypeJson::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeJson::CommercialDealer => Self::CommercialDealer,
            ShopTypeJson::Marketplace => Self::Marketplace,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateJson {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}
impl From<ProductState> for ProductStateJson {
    fn from(v: ProductState) -> Self {
        match v {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}
impl From<ProductStateJson> for ProductState {
    fn from(v: ProductStateJson) -> Self {
        match v {
            ProductStateJson::Listed => Self::Listed,
            ProductStateJson::Available => Self::Available,
            ProductStateJson::Reserved => Self::Reserved,
            ProductStateJson::Sold => Self::Sold,
            ProductStateJson::Removed => Self::Removed,
            ProductStateJson::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductLifecycleJson {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycle> for ProductLifecycleJson {
    fn from(value: ProductLifecycle) -> Self {
        match value {
            ProductLifecycle::Active => Self::Active,
            ProductLifecycle::Deleted => Self::Deleted,
        }
    }
}

impl From<ProductLifecycleJson> for ProductLifecycle {
    fn from(value: ProductLifecycleJson) -> Self {
        match value {
            ProductLifecycleJson::Active => Self::Active,
            ProductLifecycleJson::Deleted => Self::Deleted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeJson {
    min: Option<String>,
    max: Option<String>,
}
impl TryFrom<RangeQuery<OffsetDateTime>> for TimeRangeJson {
    type Error = ProductSearchJsonMappingError;
    fn try_from(v: RangeQuery<OffsetDateTime>) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductSearchJsonMappingError::FormatTimestamp)?,
            max: v
                .max
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductSearchJsonMappingError::FormatTimestamp)?,
        })
    }
}
impl TryFrom<TimeRangeJson> for RangeQuery<OffsetDateTime> {
    type Error = ProductSearchJsonMappingError;
    fn try_from(v: TimeRangeJson) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductSearchJsonMappingError::ParseTimestamp)?,
            max: v
                .max
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductSearchJsonMappingError::ParseTimestamp)?,
        })
    }
}
impl TryFrom<&ProductSearch> for ProductSearchJson {
    type Error = ProductSearchJsonMappingError;

    fn try_from(v: &ProductSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: v.language.into(),
            currency: v.currency.into(),
            product_query: v.product_query.clone(),
            enhanced_search_description: v
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_id_query: v.exclude_product_id_query.iter().copied().collect(),
            shop_name_query: v.shop_name_query.iter().cloned().collect(),
            exclude_shop_name_query: v.exclude_shop_name_query.iter().cloned().collect(),
            seller_name_query: v.seller_name_query.iter().cloned().collect(),
            exclude_seller_name_query: v.exclude_seller_name_query.iter().cloned().collect(),
            shop_slug_id_query: v.shop_slug_id_query.iter().cloned().collect(),
            exclude_shop_slug_id_query: v.exclude_shop_slug_id_query.iter().cloned().collect(),
            seller_slug_id_query: v.seller_slug_id_query.iter().cloned().collect(),
            exclude_seller_slug_id_query: v.exclude_seller_slug_id_query.iter().cloned().collect(),
            shop_type_query: v.shop_type_query.iter().copied().map(Into::into).collect(),
            country_query: v.country_query.iter().copied().collect(),
            continent_query: v.continent_query.iter().copied().map(Into::into).collect(),
            geo_address_distance_query: v.geo_address_distance_query.map(Into::into),
            price_query: v.price_query.map(|v| v.map(u64::from)),
            state_query: v.state_query.iter().copied().map(Into::into).collect(),
            lifecycle_query: v.lifecycle_query.iter().copied().map(Into::into).collect(),
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
) -> Result<ProductSearch, ProductSearchJsonMappingError> {
    let j: ProductSearchJson =
        serde_json::from_value(v).map_err(ProductSearchJsonMappingError::Deserialize)?;
    Ok(ProductSearch {
        language: j.language.into(),
        currency: j.currency.into(),
        product_query: j.product_query,
        enhanced_search_description: j
            .enhanced_search_description
            .map(EnhancedSearchDescription::from),
        exclude_product_id_query: j.exclude_product_id_query.into(),
        shop_name_query: j.shop_name_query.into(),
        exclude_shop_name_query: j.exclude_shop_name_query.into(),
        seller_name_query: j.seller_name_query.into(),
        exclude_seller_name_query: j.exclude_seller_name_query.into(),
        shop_slug_id_query: j.shop_slug_id_query.into(),
        exclude_shop_slug_id_query: j.exclude_shop_slug_id_query.into(),
        seller_slug_id_query: j.seller_slug_id_query.into(),
        exclude_seller_slug_id_query: j.exclude_seller_slug_id_query.into(),
        shop_type_query: j
            .shop_type_query
            .into_iter()
            .map(Into::into)
            .collect::<AnyOfQuery<_>>(),
        country_query: j.country_query.into(),
        continent_query: j
            .continent_query
            .into_iter()
            .map(Into::into)
            .collect::<AnyOfQuery<_>>(),
        geo_address_distance_query: j.geo_address_distance_query.map(Into::into),
        price_query: j.price_query.map(|v| v.map(Into::into)),
        state_query: j
            .state_query
            .into_iter()
            .map(Into::into)
            .collect::<AnyOfQuery<_>>(),
        lifecycle_query: j
            .lifecycle_query
            .into_iter()
            .map(Into::into)
            .collect::<AnyOfQuery<_>>(),
        created_query: j.created_query.map(TryInto::try_into).transpose()?,
        updated_query: j.updated_query.map(TryInto::try_into).transpose()?,
        auction_start_query: j.auction_start_query.map(TryInto::try_into).transpose()?,
        auction_end_query: j.auction_end_query.map(TryInto::try_into).transpose()?,
    })
}
pub(crate) fn product_search_to_json(
    v: &ProductSearch,
) -> Result<serde_json::Value, ProductSearchJsonMappingError> {
    serde_json::to_value(ProductSearchJson::try_from(v)?)
        .map_err(ProductSearchJsonMappingError::Serialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;

    #[test]
    fn should_encode_legacy_lifecycle_json_in_screaming_snake_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let lifecycle =
            ProductLifecycleJson::from(product_core::product_lifecycle::ProductLifecycle::Deleted);

        assert_eq!(
            serde_json::json!("DELETED"),
            serde_json::to_value(lifecycle)?
        );
        assert_eq!(
            product_core::product_lifecycle::ProductLifecycle::Deleted,
            serde_json::from_value::<ProductLifecycleJson>(serde_json::json!("DELETED"))?.into()
        );
        Ok(())
    }

    #[test]
    fn should_round_trip_geo_distance_query_with_legacy_json_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::De, Currency::Usd)
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
        let search = ProductSearch::new(Language::De, Currency::Usd).with_product_query(
            match "vase".try_into() {
                Ok(v) => v,
                Err(e) => panic!("bad test value: {e}"),
            },
        );
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
    fn should_serialize_every_product_search_field() {
        let json = match product_search_to_json(&ProductSearch::new(Language::En, Currency::Eur)) {
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
            ProductSearchJsonMappingError::Deserialize(_)
        ));
    }

    #[test]
    fn should_preserve_invalid_filter_row_mapping_source() {
        let search = match product_search_to_json(&ProductSearch::new(Language::En, Currency::Eur))
        {
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
            match product_search_to_json(&ProductSearch::new(Language::En, Currency::Eur)) {
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
