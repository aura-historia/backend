use common::currency::data::CurrencyData;
use common::distance::data::GeoDistanceQueryData;
use common::fx_rate_id::FxRateId;
use common::language::data::LanguageData;

use common::price::domain::MonetaryAmount;
use common::product_id::ProductId;
use common::product_lifecycle::data::ProductLifecycleData;
use common::product_state::domain::ProductState;
use common::query::any_of_query::AnyOfQuery;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::resource_state::document::ResourceStateDocument;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use product_opensearch::build_percolator_query;
use search_filter_service::ports::{CompiledSearchFilterProjection, SearchFilterView};
use serde::ser::Error as _;
use serde::{Deserialize, Serialize};
use shop_core::shop_type::ShopType;
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
    pub compiled_fx_rate_id: FxRateId,
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

impl TryFrom<&CompiledSearchFilterProjection> for SearchFilterDocument {
    type Error = serde_json::Error;

    fn try_from(projection: &CompiledSearchFilterProjection) -> Result<Self, Self::Error> {
        let view = &projection.projection.view;
        let price_filter = &projection.price_filter_plan;
        Ok(Self {
            user_search_filter_id: view.search_filter_id,
            user_id: view.user_id,
            name: view.name.clone(),
            notifications: view.notifications,
            state: view.state.into(),
            source_version: projection.projection.source_version,
            compiled_fx_rate_id: price_filter.fx_rate_id,
            search: product_search_to_value(&view.search)?,
            query: build_percolator_query(&product_service::ports::CompiledProductSearch {
                search: view.search.clone(),
                price_filter_plan: price_filter.clone(),
            })?,
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
            state: document.state.into(),
            search: product_search_from_value(document.search)?,
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
            last_hybrid_search_matched: document.last_hybrid_search_matched,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductSearchDocument {
    language: LanguageData,
    currency: CurrencyData,
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
    geo_address_distance_query: Option<GeoDistanceQueryData>,
    #[serde(rename = "price")]
    price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "state")]
    state_query: HashSet<ProductStateDocument>,
    #[serde(rename = "lifecycle")]
    lifecycle_query: HashSet<ProductLifecycleData>,
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
    use common::currency::domain::Currency;
    use common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery};
    use common::fx_rate_id::FxRateId;
    use common::language::domain::Language;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::query::range_query::RangeQuery;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use geo::core::continent::Continent;
    use isocountry::CountryCode;
    use product_service::ports::{CompiledProductSearch, ProductPriceFilterPlan};
    use search_filter_service::ports::{CompiledSearchFilterProjection, SearchFilterProjection};
    use std::collections::HashSet;
    use strum::IntoEnumIterator;
    use time::macros::datetime;

    fn price_filter() -> Result<ProductPriceFilterPlan, Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )?
        .into_persisted(1_i64.try_into()?);
        Ok(ProductPriceFilterPlan::compile(
            snapshot,
            Currency::Usd,
            Some(RangeQuery {
                min: Some(MonetaryAmount::from(100_u64)),
                max: Some(MonetaryAmount::from(999_u64)),
            }),
        )?)
    }

    fn compiled_projection(
        view: SearchFilterView,
        price_filter_plan: ProductPriceFilterPlan,
    ) -> CompiledSearchFilterProjection {
        CompiledSearchFilterProjection {
            projection: SearchFilterProjection {
                view,
                source_version: 1,
            },
            price_filter_plan,
        }
    }

    fn complete_search() -> ProductSearch {
        ProductSearch::new(Language::De, Currency::Usd)
            .with_product_query(text_query("Ming porcelain vase"))
            .with_enhanced_search_description(EnhancedSearchDescription::from("blue and white"))
            .with_exclude_product_id_query(HashSet::from([ProductId::new()]).into())
            .with_shop_name_query(HashSet::from([ShopName::from("Shop")]).into())
            .with_exclude_shop_name_query(HashSet::from([ShopName::from("Bad Shop")]).into())
            .with_seller_name_query(HashSet::from([ShopName::from("Seller")]).into())
            .with_exclude_seller_name_query(HashSet::from([ShopName::from("Bad Seller")]).into())
            .with_shop_slug_id_query(HashSet::from([ShopSlugId::from("shop")]).into())
            .with_exclude_shop_slug_id_query(HashSet::from([ShopSlugId::from("bad-shop")]).into())
            .with_seller_slug_id_query(HashSet::from([SellerSlugId::from("seller")]).into())
            .with_exclude_seller_slug_id_query(
                HashSet::from([SellerSlugId::from("bad-seller")]).into(),
            )
            .with_shop_type_query(HashSet::from([ShopType::CommercialDealer]).into())
            .with_country_query(HashSet::from([CountryCode::DEU]).into())
            .with_continent_query(HashSet::from([Continent::Europe]).into())
            .with_geo_address_distance_query(GeoDistanceQuery {
                lat: 52.52,
                lon: 13.405,
                distance: Distance {
                    amount: 10.0,
                    unit: DistanceUnit::Kilometers,
                },
            })
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(100_u64)),
                max: Some(MonetaryAmount::from(999_u64)),
            })
            .with_state_query(HashSet::from([ProductState::Available]).into())
            .with_lifecycle_query(HashSet::from([ProductLifecycle::Deleted]).into())
            .with_created_query(RangeQuery {
                min: Some(datetime!(2025-01-01 0:00 UTC)),
                max: Some(datetime!(2025-01-02 0:00 UTC)),
            })
            .with_updated_query(RangeQuery {
                min: Some(datetime!(2025-01-03 0:00 UTC)),
                max: Some(datetime!(2025-01-04 0:00 UTC)),
            })
            .with_auction_start_query(RangeQuery {
                min: Some(datetime!(2025-01-05 0:00 UTC)),
                max: Some(datetime!(2025-01-06 0:00 UTC)),
            })
            .with_auction_end_query(RangeQuery {
                min: Some(datetime!(2025-01-07 0:00 UTC)),
                max: Some(datetime!(2025-01-08 0:00 UTC)),
            })
    }

    fn sample_view(search: ProductSearch) -> SearchFilterView {
        SearchFilterView {
            search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: common::resource_state::domain::ResourceState::Active,
            search,
            embedding: Some(vec![1.0]),
            created: datetime!(2026-01-01 00:00:00 UTC),
            updated: datetime!(2026-01-02 00:00:00 UTC),
            last_hybrid_search_matched: datetime!(2026-01-03 00:00:00 UTC),
        }
    }

    fn text_query(value: &str) -> TextQuery<1> {
        match value.try_into() {
            Ok(query) => query,
            Err(error) => panic!("invalid text query: {error}"),
        }
    }

    #[test]
    fn should_round_trip_complete_product_search_document() {
        let expected = sample_view(complete_search());
        let price_filter = match price_filter() {
            Ok(price_filter) => price_filter,
            Err(error) => panic!("failed to compile price filter: {error}"),
        };
        let compiled = compiled_projection(expected.clone(), price_filter);
        let document = match SearchFilterDocument::try_from(&compiled) {
            Ok(document) => document,
            Err(error) => panic!("failed to create document: {error}"),
        };

        let actual = match SearchFilterView::try_from(document) {
            Ok(view) => view,
            Err(error) => panic!("failed to decode complete search filter document: {error}"),
        };

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_persist_every_product_search_field() {
        let price_filter = match price_filter() {
            Ok(price_filter) => price_filter,
            Err(error) => panic!("failed to compile price filter: {error}"),
        };
        let compiled = compiled_projection(sample_view(complete_search()), price_filter);
        let document = match SearchFilterDocument::try_from(&compiled) {
            Ok(document) => document,
            Err(error) => panic!("failed to create document: {error}"),
        };
        let object = match document.search.as_object() {
            Some(object) => object,
            None => panic!("product search must be an object"),
        };

        assert_eq!(PRODUCT_SEARCH_FIELDS.len(), object.len());
        for field in PRODUCT_SEARCH_FIELDS {
            assert!(object.contains_key(field));
        }
    }

    #[test]
    fn should_store_compiled_fx_rate_id_and_render_the_supplied_price_plan() {
        let view = sample_view(complete_search());
        let price_filter = match price_filter() {
            Ok(price_filter) => price_filter,
            Err(error) => panic!("failed to compile price filter: {error}"),
        };
        let compiled = compiled_projection(view.clone(), price_filter.clone());
        let document = match SearchFilterDocument::try_from(&compiled) {
            Ok(document) => document,
            Err(error) => panic!("failed to create document: {error}"),
        };
        let expected_query = match build_percolator_query(&CompiledProductSearch {
            search: view.search.clone(),
            price_filter_plan: price_filter.clone(),
        }) {
            Ok(query) => query,
            Err(error) => panic!("failed to render percolator query: {error}"),
        };

        assert_eq!(price_filter.fx_rate_id, document.compiled_fx_rate_id);
        assert_eq!(expected_query, document.query);
        assert_eq!(
            Some(&serde_json::json!(100)),
            document
                .query
                .pointer("/bool/filter/1/bool/should/0/bool/filter/0/bool/should/0/bool/filter/1/range/sourcePrice.amount/gte")
        );
    }

    #[test]
    fn should_reject_incomplete_product_search_document() {
        let price_filter = match price_filter() {
            Ok(price_filter) => price_filter,
            Err(error) => panic!("failed to compile price filter: {error}"),
        };
        let compiled = compiled_projection(sample_view(complete_search()), price_filter);
        let mut document = match SearchFilterDocument::try_from(&compiled) {
            Ok(document) => document,
            Err(error) => panic!("failed to create document: {error}"),
        };
        let object = match document.search.as_object_mut() {
            Some(object) => object,
            None => panic!("product search must be an object"),
        };
        object.remove("auctionEnd");

        assert!(SearchFilterView::try_from(document).is_err());
    }

    #[test]
    fn should_reject_unknown_product_search_document_field() {
        let price_filter = match price_filter() {
            Ok(price_filter) => price_filter,
            Err(error) => panic!("failed to compile price filter: {error}"),
        };
        let compiled = compiled_projection(sample_view(complete_search()), price_filter);
        let mut document = match SearchFilterDocument::try_from(&compiled) {
            Ok(document) => document,
            Err(error) => panic!("failed to create document: {error}"),
        };
        let object = match document.search.as_object_mut() {
            Some(object) => object,
            None => panic!("product search must be an object"),
        };
        object.insert("unexpected".to_owned(), serde_json::Value::Null);

        assert!(SearchFilterView::try_from(document).is_err());
    }
}
