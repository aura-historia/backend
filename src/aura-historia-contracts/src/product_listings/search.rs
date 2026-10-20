use application::pagination::Cursor;
use auction_core::AuctionId;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use fxrate_core::FxRateId;

use localization::Language;
use money::Currency;
use money::MonetaryAmount;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
};

use listing_source_core::ListingSourceId;
use product_listing_core::sort_product_listing_field::SortProductListingField;
use product_listing_service::use_cases::{
    ProductListingSearchCursor, SearchProductListingsRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;
use time::OffsetDateTime;

use super::product_data::{
    PersonalizedProductListingSummaryData, personalized_product_summary_data,
};
use schemars::JsonSchema;
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingSearchData {
    #[serde(default)]
    pub geography: Option<crate::geographic_search::GeographicSearchData>,
    #[serde(default)]
    pub shipping: Option<crate::shipping_search::ShippingSearchData>,
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    pub currency: Currency,
    #[serde(default, rename = "productQuery")]
    #[schemars(schema_with = "text_queries_schema")]
    pub product_listing_query: Vec<TextQuery<1>>,
    #[serde(default)]
    pub enhanced_search_description: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::product_listing_ids")]
    pub exclude_product_listing_id: HashSet<String>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    pub listing_source_id: HashSet<String>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    pub exclude_listing_source_id: HashSet<String>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::auction_ids")]
    pub auction_id: HashSet<String>,
    #[serde(default)]
    #[schemars(with = "Option<NumberRangeSchema>")]
    pub price: Option<RangeQuery<u64>>,
    #[serde(default, deserialize_with = "crate::wire::listing::availability_set")]
    #[schemars(schema_with = "crate::wire::listing::availability_set_schema")]
    pub availability: Option<HashSet<ListingAvailability>>,
    #[serde(default, deserialize_with = "crate::wire::listing::orderability_set")]
    #[schemars(schema_with = "crate::wire::listing::orderability_set_schema")]
    pub orderability: Option<HashSet<ListingOrderability>>,
    #[serde(default)]
    pub include_unspecified_availability: Option<bool>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    #[schemars(with = "DateRangeSchema")]
    pub created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    #[schemars(with = "DateRangeSchema")]
    pub updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    #[schemars(with = "DateRangeSchema")]
    pub lot_bidding_opens: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    #[schemars(with = "DateRangeSchema")]
    pub lot_scheduled_closes: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum SortProductListingFieldData {
    Score,
    Updated,
    Created,
}

impl TryFrom<&str> for SortProductListingFieldData {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(Self::Score),
            "updated" => Ok(Self::Updated),
            "created" => Ok(Self::Created),
            value => Err(format!(
                "Expected any of: 'score', 'updated', 'created'. Got: '{value}'"
            )),
        }
    }
}

impl From<SortProductListingFieldData> for SortProductListingField {
    fn from(value: SortProductListingFieldData) -> Self {
        match value {
            SortProductListingFieldData::Score => Self::Score,
            SortProductListingFieldData::Updated => Self::Updated,
            SortProductListingFieldData::Created => Self::Created,
        }
    }
}

impl TryFrom<ProductListingSearchData> for ProductListingSearch {
    type Error = ListingInputError;

    fn try_from(data: ProductListingSearchData) -> Result<Self, Self::Error> {
        Ok(Self {
            geography: data
                .geography
                .map(TryInto::try_into)
                .transpose()
                .map_err(|detail| ListingInputError::value("geography", detail))?,
            shipping: crate::shipping_search::parse(data.shipping)
                .map_err(|detail| ListingInputError::value("shipping", detail))?,
            language: data.language,
            currency: data.currency,
            product_listing_query: data.product_listing_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|error| {
                    ListingInputError::value("enhancedSearchDescription", error.to_string())
                })?,
            exclude_product_listing_id_query: parse_query_object_ids::<ProductListingId>(
                data.exclude_product_listing_id,
                "excludeProductListingId",
                "ProductListing",
            )?
            .into(),
            listing_source_id_query: parse_query_object_ids::<ListingSourceId>(
                data.listing_source_id,
                "listingSourceId",
                "ListingSource",
            )?
            .into(),
            exclude_listing_source_id_query: parse_query_object_ids::<ListingSourceId>(
                data.exclude_listing_source_id,
                "excludeListingSourceId",
                "ListingSource",
            )?
            .into(),
            auction_id_query: parse_query_object_ids::<AuctionId>(
                data.auction_id,
                "auctionId",
                "Auction",
            )?
            .into(),
            price_query: data.price.map(|range| range.map(MonetaryAmount::from)),
            availability_query: availability_query_from_parts(
                data.availability,
                data.orderability,
                data.include_unspecified_availability,
            ),
            created_query: data.created,
            updated_query: data.updated,
            lot_bidding_opens_query: data.lot_bidding_opens,
            lot_scheduled_closes_query: data.lot_scheduled_closes,
        })
    }
}

fn parse_query_object_ids<T>(
    values: HashSet<String>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<HashSet<T>, ListingInputError>
where
    T: FromStr + Eq + Hash,
{
    values
        .into_iter()
        .map(|value| {
            value
                .parse()
                .map_err(|_| ListingInputError::identifier(field, semantic_type))
        })
        .collect()
}

fn availability_query_from_parts(
    availability: Option<HashSet<ListingAvailability>>,
    orderability: Option<HashSet<ListingOrderability>>,
    include_unspecified: Option<bool>,
) -> Option<ListingAvailabilityQuery> {
    if availability.is_none() && orderability.is_none() && include_unspecified.is_none() {
        None
    } else {
        Some(ListingAvailabilityQuery {
            any_of: availability.unwrap_or_default().into(),
            orderability: orderability.unwrap_or_default().into(),
            include_unspecified: include_unspecified.unwrap_or(false),
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CursoredProductListingsData {
    pub items: Vec<PersonalizedProductListingSummaryData>,
    #[schemars(range(max = u64::MAX))]
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_after: Option<ProductListingSearchCursorData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = u64::MAX))]
    pub total: Option<u64>,
}

/// Public encoding of the ProductListing-owned opaque continuation cursor.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingSearchCursorData {
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    pub fx_rate_id: FxRateId,
    pub search_after: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingSearchCursorInputData {
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    pub fx_rate_id: String,
    pub search_after: Value,
}

impl From<ProductListingSearchCursor> for ProductListingSearchCursorData {
    fn from(cursor: ProductListingSearchCursor) -> Self {
        Self {
            fx_rate_id: cursor.fx_rate_id,
            search_after: cursor.search_after,
        }
    }
}

/// Query options shared by REST extraction and MCP's combined argument envelope.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingSearchOptionsData {
    #[schemars(with = "Option<SortProductListingFieldData>")]
    pub sort: Option<String>,
    #[schemars(schema_with = "sort_order_schema")]
    pub order: Option<String>,
    /// Values are clamped to the existing service-supported range 1..=100.
    #[schemars(range(max = u64::MAX))]
    pub size: Option<u64>,
    pub search_after: Option<ProductListingSearchCursorInputData>,
}
impl ProductListingSearchOptionsData {
    pub fn into_request(
        self,
        data: ProductListingSearchData,
    ) -> Result<SearchProductListingsRequest, ListingInputError> {
        Ok(SearchProductListingsRequest {
            search: data.try_into()?,
            sort: parse_sort(self.sort.as_deref(), self.order.as_deref())?,
            cursor: parse_cursor(self.size, self.search_after)?,
        })
    }
}
pub fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortProductListingField>>, ListingInputError> {
    let sort = sort
        .map(|sort| {
            SortProductListingFieldData::try_from(sort).map_err(|detail| ListingInputError {
                field: "sort",
                kind: ListingInputErrorKind::Sort,
                detail,
            })
        })
        .transpose()?;
    let order = order
        .map(|order| {
            SortOrder::try_from(order).map_err(|detail| ListingInputError {
                field: "order",
                kind: ListingInputErrorKind::Order,
                detail: detail.to_string(),
            })
        })
        .transpose()?;
    match (sort, order) {
        (None, None) => Ok(None),
        (Some(sort), order) => Ok(Some(Sort {
            sort: sort.into(),
            order: order.unwrap_or(SortOrder::Desc),
        })),
        (None, Some(_)) => Err(ListingInputError {
            field: "sort",
            kind: ListingInputErrorKind::Sort,
            detail: "'sort' is required when 'order' is supplied.".into(),
        }),
    }
}
pub fn parse_cursor(
    size: Option<u64>,
    search_after: Option<ProductListingSearchCursorInputData>,
) -> Result<Option<Cursor<ProductListingSearchCursor>>, ListingInputError> {
    let search_after = search_after
        .map(|cursor| {
            Ok(ProductListingSearchCursor {
                fx_rate_id: cursor
                    .fx_rate_id
                    .parse()
                    .map_err(|_| ListingInputError::identifier("searchAfter", "FxRate"))?,
                search_after: cursor.search_after,
            })
        })
        .transpose()?;
    Ok(if size.is_some() || search_after.is_some() {
        Some(Cursor {
            size: size
                .map(|n| n.clamp(1, 100))
                .unwrap_or_else(|| Cursor::<ProductListingSearchCursor>::default().size),
            search_after,
        })
    } else {
        None
    })
}
impl From<product_listing_service::use_cases::SearchProductListingsResult>
    for CursoredProductListingsData
{
    fn from(result: product_listing_service::use_cases::SearchProductListingsResult) -> Self {
        let items = result
            .items
            .into_iter()
            .map(personalized_product_summary_data)
            .collect::<Vec<_>>();
        Self {
            size: items.len() as u64,
            items,
            search_after: result.cursor.search_after.map(Into::into),
            total: result.total,
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct ListingInputError {
    pub field: &'static str,
    pub kind: ListingInputErrorKind,
    pub detail: String,
}
#[derive(Debug)]
pub enum ListingInputErrorKind {
    Value,
    Identifier,
    Sort,
    Order,
}
impl ListingInputError {
    fn value(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field,
            kind: ListingInputErrorKind::Value,
            detail: detail.into(),
        }
    }
    fn identifier(field: &'static str, semantic_type: &'static str) -> Self {
        Self {
            field,
            kind: ListingInputErrorKind::Identifier,
            detail: format!("must be a valid {semantic_type} ID"),
        }
    }
}
fn text_queries_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"array","items":{"type":"string","minLength":1}})
}
fn sort_order_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"anyOf":[{"type":"string","enum":[SortOrder::Asc.as_str(),SortOrder::Desc.as_str()]},{"type":"null"}]})
}
#[derive(JsonSchema)]
pub struct NumberRangeSchema {
    #[schemars(range(max = u64::MAX))]
    pub min: Option<u64>,
    #[schemars(range(max = u64::MAX))]
    pub max: Option<u64>,
}
#[derive(JsonSchema)]
pub struct DateRangeSchema {
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub min: Option<String>,
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub max: Option<String>,
}
