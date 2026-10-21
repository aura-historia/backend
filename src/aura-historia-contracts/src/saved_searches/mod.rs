//! Shared saved-search contracts and pure use-case conversions.
mod code_sets;
mod input;
mod matches;
mod presentation;
use self::input::{non_nullable_patch, parse_body_object_id};
use crate::patch_value::{PatchValue, clearable};
pub use input::{SavedSearchInputError, SavedSearchInputErrorKind};
pub use matches::{
    MatchCursorOutputError, SearchFilterMatchesData, SearchFilterMatchesQuery, match_cursor_schema,
    matches_cursor_value, parse_matches_cursor,
};
pub use presentation::{PaginatedData, SearchFilterData, SearchFilterMatchData};
use schemars::JsonSchema;

use application::patch_field::PatchField;
use auction_core::AuctionId;

use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use localization::Language;
use money::Currency;

use listing_source_core::ListingSourceId;
use money::MonetaryAmount;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;

use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;

use search_filter_service::use_cases::ProductListingSearchPatch;
use serde::{Deserialize, Serialize};

use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PatchSearchFilterStateData {
    Active,
    InactiveByUser,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSearchFilterData {
    #[serde(default)]
    pub accept_shipping_destination: Option<AcceptShippingDestinationData>,
    #[schemars(with = "String")]
    pub name: UserSearchFilterName,
    #[serde(default = "default_notifications")]
    pub notifications: bool,
    pub search: ProductListingSearchData,
}

fn default_notifications() -> bool {
    true
}

pub type UpdateSearchFilterFields = (
    PatchField<UserSearchFilterName>,
    PatchField<bool>,
    PatchField<SearchFilterState>,
    ProductListingSearchPatch,
);

#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSearchFilterData {
    #[serde(default)]
    pub accept_shipping_destination: Option<AcceptShippingDestinationData>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub name: Option<UserSearchFilterName>,
    #[serde(default)]
    pub notifications: Option<bool>,
    #[serde(default)]
    pub state: Option<PatchSearchFilterStateData>,
    #[serde(default)]
    pub search: Option<ProductListingSearchPatchData>,
}

impl UpdateSearchFilterData {
    pub fn into_fields(self) -> Result<UpdateSearchFilterFields, SavedSearchInputError> {
        let search = match self.search {
            Some(search) => search.try_into_patch()?,
            None => ProductListingSearchPatch::default(),
        };

        Ok((
            self.name.map_or(PatchField::Unchanged, PatchField::Set),
            self.notifications
                .map_or(PatchField::Unchanged, PatchField::Set),
            self.state
                .map(search_filter_state)
                .map_or(PatchField::Unchanged, PatchField::Set),
            search,
        ))
    }
}

#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(transform = code_sets::atomic_availability_patch)]
pub struct ProductListingSearchPatchData {
    #[serde(default)]
    geography: PatchValue<crate::geographic_search::GeographicSearchData>,
    #[serde(default)]
    shipping: PatchValue<crate::shipping_search::ShippingSearchData>,
    #[serde(default)]
    #[serde(deserialize_with = "code_sets::language_patch")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    language: PatchValue<Language>,
    #[serde(default)]
    #[serde(deserialize_with = "code_sets::currency_patch")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: PatchValue<Currency>,
    #[serde(rename = "productQuery", default)]
    #[schemars(schema_with = "code_sets::text_queries")]
    product_listing_query: PatchValue<Vec<TextQuery<1>>>,
    #[serde(rename = "enhancedSearchDescription", default)]
    enhanced_search_description: PatchValue<String>,
    #[serde(rename = "listingSourceId", default)]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    listing_source_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "excludeListingSourceId", default)]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    exclude_listing_source_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "auctionId", default)]
    #[schemars(schema_with = "code_sets::nullable_auction_ids")]
    auction_id_query: PatchValue<HashSet<String>>,
    #[serde(rename = "price", default)]
    #[schemars(with = "Option<crate::product_listings::search::NumberRangeSchema>")]
    price_query: PatchValue<RangeQuery<u64>>,
    #[serde(rename = "availability", default)]
    #[serde(deserialize_with = "code_sets::availability_patch")]
    #[schemars(schema_with = "crate::wire::listing::availability_set_schema")]
    availability_query: PatchValue<HashSet<ListingAvailability>>,
    #[serde(rename = "orderability", default)]
    #[serde(deserialize_with = "code_sets::listing_orderability_patch_set::deserialize")]
    #[schemars(schema_with = "crate::wire::listing::orderability_set_schema")]
    orderability_query: PatchValue<HashSet<ListingOrderability>>,
    #[serde(rename = "includeUnspecifiedAvailability", default)]
    include_unspecified_availability: PatchValue<bool>,
    #[serde(
        rename = "created",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    #[schemars(with = "Option<crate::product_listings::search::DateRangeSchema>")]
    created_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    #[schemars(with = "Option<crate::product_listings::search::DateRangeSchema>")]
    updated_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotBiddingOpens",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    #[schemars(with = "Option<crate::product_listings::search::DateRangeSchema>")]
    lot_bidding_opens_query: PatchValue<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotScheduledCloses",
        default,
        deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
    )]
    #[schemars(with = "Option<crate::product_listings::search::DateRangeSchema>")]
    lot_scheduled_closes_query: PatchValue<RangeQuery<OffsetDateTime>>,
}

impl ProductListingSearchPatchData {
    fn try_into_patch(self) -> Result<ProductListingSearchPatch, SavedSearchInputError> {
        Ok(ProductListingSearchPatch {
            geography: geographic_patch(self.geography, "search.geography")?,
            shipping: match self.shipping {
                PatchValue::Omitted => PatchField::Unchanged,
                PatchValue::Null => PatchField::Clear,
                PatchValue::Value(d) => match parse_shipping(Some(d))? {
                    Some(q) => PatchField::Set(q),
                    None => PatchField::Clear,
                },
            },
            language: non_nullable_patch(self.language, "search.language")?,
            currency: non_nullable_patch(self.currency, "search.currency")?,
            product_listing_query: non_nullable_patch(
                self.product_listing_query,
                "search.productQuery",
            )?,
            enhanced_search_description: match self.enhanced_search_description {
                PatchValue::Omitted => PatchField::Unchanged,
                PatchValue::Null => PatchField::Clear,
                PatchValue::Value(value) => PatchField::Set(
                    EnhancedSearchDescription::try_from(value).map_err(|error| {
                        SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)
                            .with_detail(error.to_string())
                    })?,
                ),
            },
            listing_source_id_query: non_nullable_patch(
                parse_body_object_id_patch::<ListingSourceId>(
                    self.listing_source_id_query,
                    "search.listingSourceId",
                    "ListingSource",
                )?
                .map(Into::into),
                "search.listingSourceId",
            )?,
            exclude_listing_source_id_query: non_nullable_patch(
                parse_body_object_id_patch::<ListingSourceId>(
                    self.exclude_listing_source_id_query,
                    "search.excludeListingSourceId",
                    "ListingSource",
                )?
                .map(Into::into),
                "search.excludeListingSourceId",
            )?,
            auction_id_query: clearable(
                parse_body_object_id_patch::<AuctionId>(
                    self.auction_id_query,
                    "search.auctionId",
                    "Auction",
                )?
                .map(Into::into),
            ),
            price_query: clearable(
                self.price_query
                    .map(|query| query.map(MonetaryAmount::from)),
            ),
            availability_query: availability_query_patch(
                self.availability_query,
                self.orderability_query,
                self.include_unspecified_availability,
            )?,
            created_query: clearable(self.created_query),
            updated_query: clearable(self.updated_query),
            lot_bidding_opens_query: clearable(self.lot_bidding_opens_query),
            lot_scheduled_closes_query: clearable(self.lot_scheduled_closes_query),
        })
    }
}

fn parse_body_object_ids<T>(
    values: HashSet<String>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<HashSet<T>, SavedSearchInputError>
where
    T: FromStr + Eq + Hash,
{
    values
        .into_iter()
        .map(|value| parse_body_object_id(&value, field, semantic_type))
        .collect()
}

fn parse_body_object_id_patch<T>(
    values: PatchValue<HashSet<String>>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<PatchValue<HashSet<T>>, SavedSearchInputError>
where
    T: FromStr + Eq + Hash,
{
    match values {
        PatchValue::Omitted => Ok(PatchValue::Omitted),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Value(values) => {
            parse_body_object_ids(values, field, semantic_type).map(PatchValue::Value)
        }
    }
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

fn availability_query_patch(
    availability: PatchValue<HashSet<ListingAvailability>>,
    orderability: PatchValue<HashSet<ListingOrderability>>,
    include_unspecified: PatchValue<bool>,
) -> Result<PatchField<ListingAvailabilityQuery>, SavedSearchInputError> {
    match (availability, orderability, include_unspecified) {
        (PatchValue::Omitted, PatchValue::Omitted, PatchValue::Omitted) => {
            Ok(PatchField::Unchanged)
        }
        (PatchValue::Null, PatchValue::Null, PatchValue::Null) => Ok(PatchField::Clear),
        (
            PatchValue::Value(availability),
            PatchValue::Value(orderability),
            PatchValue::Value(include_unspecified),
        ) => Ok(PatchField::Set(ListingAvailabilityQuery {
            any_of: availability.into(),
            orderability: orderability.into(),
            include_unspecified,
        })),
        _ => Err(
            SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue).with_detail(
                "Availability query fields must be supplied together, or all null to clear the query.",
            ),
        ),
    }
}

#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSearchFilterMatchFeedbackData {
    #[serde(default)]
    pub feedback: PatchValue<bool>,
}

impl UpdateSearchFilterMatchFeedbackData {
    pub fn feedback(self) -> PatchField<bool> {
        clearable(self.feedback)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(bound(
    deserialize = "ProductId: Deserialize<'de> + Eq + Hash, SourceId: Deserialize<'de> + Eq + Hash, AuctionIdValue: Deserialize<'de> + Eq + Hash"
))]
#[schemars(bound = "")]
pub struct ProductListingSearchData<ProductId = String, SourceId = String, AuctionIdValue = String>
{
    #[serde(default, skip_serializing_if = "Option::is_none")]
    geography: Option<crate::geographic_search::GeographicSearchData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shipping: Option<crate::shipping_search::ShippingSearchData>,
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: Currency,
    #[serde(
        rename = "productQuery",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    #[schemars(schema_with = "code_sets::text_queries")]
    product_listing_query: Vec<TextQuery<1>>,
    #[serde(
        rename = "enhancedSearchDescription",
        skip_serializing_if = "Option::is_none",
        default
    )]
    enhanced_search_description: Option<String>,
    #[serde(
        rename = "excludeProductListingId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[schemars(schema_with = "crate::wire::schema::product_listing_ids")]
    exclude_product_listing_id_query: HashSet<ProductId>,
    #[serde(
        rename = "listingSourceId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    listing_source_id_query: HashSet<SourceId>,
    #[serde(
        rename = "excludeListingSourceId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[schemars(schema_with = "crate::wire::schema::listing_source_ids")]
    exclude_listing_source_id_query: HashSet<SourceId>,
    #[serde(
        rename = "auctionId",
        skip_serializing_if = "HashSet::is_empty",
        default
    )]
    #[schemars(schema_with = "crate::wire::schema::auction_ids")]
    auction_id_query: HashSet<AuctionIdValue>,
    #[serde(rename = "price", skip_serializing_if = "Option::is_none", default)]
    #[schemars(with = "Option<crate::product_listings::search::NumberRangeSchema>")]
    price_query: Option<RangeQuery<u64>>,
    #[serde(
        rename = "availability",
        skip_serializing_if = "Option::is_none",
        default,
        with = "code_sets::listing_availability_set_option"
    )]
    #[schemars(schema_with = "crate::wire::listing::availability_set_schema")]
    availability_query: Option<HashSet<ListingAvailability>>,
    #[serde(
        rename = "orderability",
        skip_serializing_if = "Option::is_none",
        default,
        with = "code_sets::listing_orderability_set_option"
    )]
    #[schemars(schema_with = "crate::wire::listing::orderability_set_schema")]
    orderability_query: Option<HashSet<ListingOrderability>>,
    #[serde(
        rename = "includeUnspecifiedAvailability",
        skip_serializing_if = "Option::is_none",
        default
    )]
    include_unspecified_availability: Option<bool>,
    #[serde(
        rename = "created",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::product_listings::search::DateRangeSchema")]
    created_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "updated",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::product_listings::search::DateRangeSchema")]
    updated_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotBiddingOpens",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::product_listings::search::DateRangeSchema")]
    lot_bidding_opens_query: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        rename = "lotScheduledCloses",
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "crate::product_listings::search::DateRangeSchema")]
    lot_scheduled_closes_query: Option<RangeQuery<OffsetDateTime>>,
}

impl TryFrom<ProductListingSearchData<String, String, String>> for ProductListingSearch {
    type Error = SavedSearchInputError;

    fn try_from(
        data: ProductListingSearchData<String, String, String>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            geography: parse_geography(data.geography, "search.geography")?,
            shipping: parse_shipping(data.shipping)?,
            language: data.language,
            currency: data.currency,
            product_listing_query: data.product_listing_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|error| {
                    SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)
                        .with_body_field("search.enhancedSearchDescription")
                        .with_detail(error.to_string())
                })?,
            exclude_product_listing_id_query: parse_body_object_ids::<ProductListingId>(
                data.exclude_product_listing_id_query,
                "search.excludeProductListingId",
                "ProductListing",
            )?
            .into(),
            listing_source_id_query: parse_body_object_ids::<ListingSourceId>(
                data.listing_source_id_query,
                "search.listingSourceId",
                "ListingSource",
            )?
            .into(),
            exclude_listing_source_id_query: parse_body_object_ids::<ListingSourceId>(
                data.exclude_listing_source_id_query,
                "search.excludeListingSourceId",
                "ListingSource",
            )?
            .into(),
            auction_id_query: parse_body_object_ids::<AuctionId>(
                data.auction_id_query,
                "search.auctionId",
                "Auction",
            )?
            .into(),
            price_query: data
                .price_query
                .map(|query| query.map(MonetaryAmount::from)),
            availability_query: availability_query_from_parts(
                data.availability_query,
                data.orderability_query,
                data.include_unspecified_availability,
            ),
            created_query: data.created_query,
            updated_query: data.updated_query,
            lot_bidding_opens_query: data.lot_bidding_opens_query,
            lot_scheduled_closes_query: data.lot_scheduled_closes_query,
        })
    }
}

impl From<ProductListingSearch>
    for ProductListingSearchData<ProductListingId, ListingSourceId, AuctionId>
{
    fn from(search: ProductListingSearch) -> Self {
        Self {
            geography: search.geography.map(Into::into),
            shipping: search.shipping.map(Into::into),
            language: search.language,
            currency: search.currency,
            product_listing_query: search.product_listing_query,
            enhanced_search_description: search.enhanced_search_description.map(Into::into),
            exclude_product_listing_id_query: search.exclude_product_listing_id_query.into(),
            listing_source_id_query: search.listing_source_id_query.into(),
            exclude_listing_source_id_query: search.exclude_listing_source_id_query.into(),
            auction_id_query: search.auction_id_query.into(),
            price_query: search.price_query.map(|query| query.map(u64::from)),
            availability_query: search
                .availability_query
                .as_ref()
                .map(|query| query.any_of.iter().copied().collect()),
            orderability_query: search
                .availability_query
                .as_ref()
                .map(|query| query.orderability.iter().copied().collect()),
            include_unspecified_availability: search
                .availability_query
                .as_ref()
                .map(|query| query.include_unspecified),
            created_query: search.created_query,
            updated_query: search.updated_query,
            lot_bidding_opens_query: search.lot_bidding_opens_query,
            lot_scheduled_closes_query: search.lot_scheduled_closes_query,
        }
    }
}

fn geographic_patch(
    value: PatchValue<crate::geographic_search::GeographicSearchData>,
    field: &'static str,
) -> Result<PatchField<geo::core::discovery::GeographicQuery>, SavedSearchInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(d) => parse_geography(Some(d), field)?
            .map(PatchField::Set)
            .ok_or_else(|| SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)),
    }
}

#[cfg(test)]
mod tests;

fn search_filter_state(state: PatchSearchFilterStateData) -> SearchFilterState {
    match state {
        PatchSearchFilterStateData::Active => SearchFilterState::Active,
        PatchSearchFilterStateData::InactiveByUser => SearchFilterState::InactiveByUser,
    }
}

/// Optional acceptance guard. The submitted country must equal the authorized address or
/// preference's asserted country. Neither the reference nor provider evidence is retained.
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum AcceptShippingDestinationData {
    Address {
        #[serde(rename = "addressId")]
        #[schemars(schema_with = "code_sets::address_id")]
        address_id: user_core::user_address::UserAddressId,
    },
    Preference {},
}
impl std::fmt::Debug for AcceptShippingDestinationData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcceptShippingDestinationData(<redacted>)")
    }
}
impl From<AcceptShippingDestinationData>
    for search_filter_service::shipping_destination::AcceptShippingDestination
{
    fn from(d: AcceptShippingDestinationData) -> Self {
        match d {
            AcceptShippingDestinationData::Address { address_id } => Self::Address(address_id),
            AcceptShippingDestinationData::Preference {} => Self::Preference,
        }
    }
}

fn parse_geography(
    data: Option<crate::geographic_search::GeographicSearchData>,
    field: &'static str,
) -> Result<Option<geo::core::discovery::GeographicQuery>, SavedSearchInputError> {
    data.map(TryInto::try_into).transpose().map_err(|detail| {
        SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)
            .with_body_field(field)
            .with_detail(detail)
    })
}
fn parse_shipping(
    data: Option<crate::shipping_search::ShippingSearchData>,
) -> Result<Option<product_listing_core::shipping_search::ShippingSearch>, SavedSearchInputError> {
    crate::shipping_search::parse(data).map_err(|detail| {
        SavedSearchInputError::new(SavedSearchInputErrorKind::BodyValue)
            .with_body_field("search.shipping")
            .with_detail(detail)
    })
}

impl CreateSearchFilterData {
    pub fn into_command(
        self,
        user_id: user_core::user_id::UserId,
    ) -> Result<search_filter_service::use_cases::CreateSearchFilterCommand, SavedSearchInputError>
    {
        Ok(
            search_filter_service::use_cases::CreateSearchFilterCommand {
                user_id,
                accept_shipping_destination: self.accept_shipping_destination.map(Into::into),
                name: self.name,
                notifications: self.notifications,
                search: self.search.try_into()?,
            },
        )
    }
}
impl UpdateSearchFilterData {
    pub fn into_command(
        self,
        user_id: user_core::user_id::UserId,
        search_filter_id: UserSearchFilterId,
    ) -> Result<
        search_filter_service::use_cases::UpdateOwnedSearchFilterCommand,
        SavedSearchInputError,
    > {
        let accept_shipping_destination = self.accept_shipping_destination.clone().map(Into::into);
        let (name, notifications, state, search) = self.into_fields()?;
        Ok(
            search_filter_service::use_cases::UpdateOwnedSearchFilterCommand {
                user_id,
                search_filter_id,
                accept_shipping_destination,
                name,
                notifications,
                state,
                search,
            },
        )
    }
}
