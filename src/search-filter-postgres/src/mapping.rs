use common::event_id::EventId;
use common::language::data::LanguageData;
use common::product_id::ProductId;
use common::product_lifecycle::data::ProductLifecycleData;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::resource_state::domain::ResourceState;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use common::{currency::data::CurrencyData, error::boxed::box_error};

use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
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

pub(crate) const FILTER_COLUMNS: &str = "user_search_filter_id, user_id, name, notifications, state, search, embedding, created, updated, last_hybrid_search_matched, version";
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
    pub last_hybrid_search_matched: OffsetDateTime,
    pub version: i64,
}
impl FilterRow {
    pub(crate) fn into_persisted(
        self,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let created = self.created;
        let updated = self.updated;
        let last_hybrid_search_matched = self.last_hybrid_search_matched;
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
            last_hybrid_search_matched,
            version: self.version,
        })
    }
    pub(crate) fn into_view(self) -> Result<SearchFilterView, SearchFilterReadError> {
        let created = self.created;
        let updated = self.updated;
        let last_hybrid_search_matched = self.last_hybrid_search_matched;
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
            last_hybrid_search_matched,
        })
    }

    pub(crate) fn into_projection(
        self,
    ) -> Result<SearchFilterProjection, SearchFilterIndexReadError> {
        let source_version = self.version;
        let created = self.created;
        let updated = self.updated;
        let last_hybrid_search_matched = self.last_hybrid_search_matched;
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
            last_hybrid_search_matched,
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

pub(crate) fn format_state(value: ResourceState) -> &'static str {
    match value {
        ResourceState::Active => "ACTIVE",
        ResourceState::InactiveByUser => "INACTIVE_BY_USER",
        ResourceState::InactiveByRestrictedPlan => "INACTIVE_BY_RESTRICTED_PLAN",
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
                    fx_rate_id: common::fx_rate_id::FxRateId::from(fx_rate_id),
                })
                .ok_or(SearchFilterRowMappingError::InvalidPriceMatchValuation)
                .map(Some)
        }
        _ => Err(SearchFilterRowMappingError::InvalidPriceMatchValuation),
    }
}

fn state(v: &str) -> Result<ResourceState, SearchFilterRowMappingError> {
    match v {
        "ACTIVE" => Ok(ResourceState::Active),
        "INACTIVE_BY_USER" => Ok(ResourceState::InactiveByUser),
        "INACTIVE_BY_RESTRICTED_PLAN" => Ok(ResourceState::InactiveByRestrictedPlan),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductSearchJson {
    language: LanguageData,
    currency: CurrencyData,
    product_query: Vec<common::query::text_query::TextQuery<1>>,
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
    geo_address_distance_query: Option<common::distance::data::GeoDistanceQueryData>,
    price_query: Option<RangeQuery<u64>>,
    state_query: HashSet<ProductStateJson>,
    lifecycle_query: HashSet<ProductLifecycleData>,
    created_query: Option<TimeRangeJson>,
    updated_query: Option<TimeRangeJson>,
    auction_start_query: Option<TimeRangeJson>,
    auction_end_query: Option<TimeRangeJson>,
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
fn product_search_from_json(
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
    use common::currency::domain::Currency;
    use common::language::domain::Language;
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
            last_hybrid_search_matched: OffsetDateTime::UNIX_EPOCH,
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
