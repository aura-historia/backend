use crate::{
    ports::{SearchFilterMatchView, SearchFilterView},
    use_cases::{
        DeleteOwnedSearchFilterResult, UpdateOwnedSearchFilterResult,
        UpdateSearchFilterMatchFeedbackResult,
    },
};
use application::{
    error::{BoxError, static_error},
    idempotency::{self as retry, CommandOutcome},
};
use auction_core::AuctionId;
use domain_primitives::query::range_query::RangeQuery;
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability,
    listing_orderability::ListingOrderability,
    product_listing_id::ProductListingId,
    product_listing_search::{
        EnhancedSearchDescription, EnhancedSearchDescriptionError, ListingAvailabilityQuery,
        ProductListingSearch,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, error::Error, fmt};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
#[derive(Debug)]
pub(crate) enum ProductListingSearchJsonMappingError {
    Geography(geo::data::discovery_data::InvalidGeographicQueryData),
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    FormatTimestamp(time::error::Format),
    ParseTimestamp(time::error::Parse),
    EnhancedSearchDescription(EnhancedSearchDescriptionError),
    ObjectId(domain_primitives::object_id::ObjectIdError),
}

impl fmt::Display for ProductListingSearchJsonMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Geography(_) => formatter.write_str("invalid geographic predicate"),
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
            Self::ObjectId(_) => {
                formatter.write_str("persisted product search object ID is invalid")
            }
        }
    }
}

impl Error for ProductListingSearchJsonMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Geography(source) => Some(source),
            Self::Serialize(source) | Self::Deserialize(source) => Some(source),
            Self::FormatTimestamp(source) => Some(source),
            Self::ParseTimestamp(source) => Some(source),
            Self::EnhancedSearchDescription(source) => Some(source),
            Self::ObjectId(source) => Some(source),
        }
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

mod canonical_uuid_set {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<uuid::Uuid>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(values.iter().map(uuid::Uuid::to_string))
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<uuid::Uuid>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .into_iter()
            .map(|value| {
                let uuid = uuid::Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
                if uuid.to_string() != value {
                    return Err(serde::de::Error::custom(
                        "persisted UUID must use canonical hyphenated lowercase text",
                    ));
                }
                Ok(uuid)
            })
            .collect()
    }
}

mod listing_availability {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ListingAvailability>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ListingAvailability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashSet<ListingAvailability>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ListingAvailability::from_code)
    }
}

mod listing_orderability {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ListingOrderability>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ListingOrderability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashSet<ListingOrderability>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ListingOrderability::from_code)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingSearchJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    geography: Option<geo::data::discovery_data::GeographicQueryData>,
    #[serde(with = "language")]
    language: Language,
    #[serde(with = "currency")]
    currency: Currency,
    product_listing_query: Vec<domain_primitives::query::text_query::TextQuery<1>>,
    enhanced_search_description: Option<String>,
    #[serde(with = "canonical_uuid_set")]
    exclude_product_listing_id_query: HashSet<uuid::Uuid>,
    #[serde(with = "canonical_uuid_set")]
    listing_source_id_query: HashSet<uuid::Uuid>,
    #[serde(with = "canonical_uuid_set")]
    exclude_listing_source_id_query: HashSet<uuid::Uuid>,
    #[serde(with = "canonical_uuid_set")]
    auction_id_query: HashSet<uuid::Uuid>,
    price_query: Option<RangeQuery<u64>>,
    availability_query: Option<ListingAvailabilityQueryJson>,
    created_query: Option<TimeRangeJson>,
    updated_query: Option<TimeRangeJson>,
    lot_bidding_opens_query: Option<TimeRangeJson>,
    lot_scheduled_closes_query: Option<TimeRangeJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingAvailabilityQueryJson {
    #[serde(with = "listing_availability")]
    availability: HashSet<ListingAvailability>,
    #[serde(with = "listing_orderability")]
    orderability: HashSet<ListingOrderability>,
    include_unspecified: bool,
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
            geography: v.geography.as_ref().map(Into::into),
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
                .map(ProductListingId::into_uuid)
                .collect(),
            listing_source_id_query: v
                .listing_source_id_query
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect(),
            exclude_listing_source_id_query: v
                .exclude_listing_source_id_query
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect(),
            auction_id_query: v
                .auction_id_query
                .iter()
                .copied()
                .map(AuctionId::into_uuid)
                .collect(),
            price_query: v.price_query.map(|v| v.map(u64::from)),
            availability_query: v.availability_query.as_ref().map(|query| {
                ListingAvailabilityQueryJson {
                    availability: query.any_of.iter().copied().collect(),
                    orderability: query.orderability.iter().copied().collect(),
                    include_unspecified: query.include_unspecified,
                }
            }),
            created_query: v.created_query.map(TimeRangeJson::try_from).transpose()?,
            updated_query: v.updated_query.map(TimeRangeJson::try_from).transpose()?,
            lot_bidding_opens_query: v
                .lot_bidding_opens_query
                .map(TimeRangeJson::try_from)
                .transpose()?,
            lot_scheduled_closes_query: v
                .lot_scheduled_closes_query
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
    let search = ProductListingSearch {
        geography: j
            .geography
            .map(TryInto::try_into)
            .transpose()
            .map_err(ProductListingSearchJsonMappingError::Geography)?,
        language: j.language,
        currency: j.currency,
        product_listing_query: j.product_listing_query,
        enhanced_search_description: j
            .enhanced_search_description
            .map(EnhancedSearchDescription::try_from)
            .transpose()
            .map_err(ProductListingSearchJsonMappingError::EnhancedSearchDescription)?,
        exclude_product_listing_id_query: j
            .exclude_product_listing_id_query
            .into_iter()
            .map(ProductListingId::try_from)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        listing_source_id_query: j
            .listing_source_id_query
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        exclude_listing_source_id_query: j
            .exclude_listing_source_id_query
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        auction_id_query: j
            .auction_id_query
            .into_iter()
            .map(AuctionId::try_from)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        price_query: j.price_query.map(|v| v.map(Into::into)),
        availability_query: j.availability_query.map(|query| ListingAvailabilityQuery {
            any_of: query.availability.into(),
            orderability: query.orderability.into(),
            include_unspecified: query.include_unspecified,
        }),
        created_query: j.created_query.map(TryInto::try_into).transpose()?,
        updated_query: j.updated_query.map(TryInto::try_into).transpose()?,
        lot_bidding_opens_query: j
            .lot_bidding_opens_query
            .map(TryInto::try_into)
            .transpose()?,
        lot_scheduled_closes_query: j
            .lot_scheduled_closes_query
            .map(TryInto::try_into)
            .transpose()?,
    };

    Ok(search)
}
pub(crate) fn product_search_to_json(
    v: &ProductListingSearch,
) -> Result<serde_json::Value, ProductListingSearchJsonMappingError> {
    serde_json::to_value(ProductListingSearchJson::try_from(v)?)
        .map_err(ProductListingSearchJsonMappingError::Serialize)
}

impl CommandOutcome for SearchFilterView {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"search_filter_id":self.search_filter_id,"user_id":self.user_id,"name":self.name.as_ref(),"notifications":self.notifications,"state":self.state.as_str(),"search":product_search_to_json(&self.search)?,"embedding":self.embedding,"created":retry::time_value(self.created)?,"updated":retry::time_value(self.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        let name: String = retry::field(&v, "name")?;
        if name.len() > 255 {
            return Err(static_error("invalid filter name"));
        }
        let embedding: Option<Vec<f32>> = retry::field(&v, "embedding")?;
        if embedding
            .as_ref()
            .is_some_and(|v| v.len() != 768 || v.iter().any(|v| !v.is_finite()))
        {
            return Err(static_error("invalid embedding"));
        }
        Ok(Self {
            search_filter_id: retry::field(&v, "search_filter_id")?,
            user_id: retry::field(&v, "user_id")?,
            name: name.into(),
            notifications: retry::field(&v, "notifications")?,
            state: search_filter_core::search_filter_state::SearchFilterState::from_code(
                &retry::field::<String>(&v, "state")?,
            )
            .ok_or_else(|| static_error("invalid filter state"))?,
            search: product_search_from_json(retry::field(&v, "search")?)?,
            embedding,
            created: retry::timestamp(&v, "created")?,
            updated: retry::timestamp(&v, "updated")?,
        })
    }
}
impl CommandOutcome for UpdateOwnedSearchFilterResult {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.filter.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        self.filter.encode()
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            filter: SearchFilterView::decode(v)?,
        })
    }
}
application::command_outcome!(unit DeleteOwnedSearchFilterResult);
impl CommandOutcome for UpdateSearchFilterMatchFeedbackResult {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.search_filter_match.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        let v = &self.search_filter_match;
        Ok(
            json!({"user_id":v.user_id,"search_filter_id":v.search_filter_id,"search_filter_name":v.search_filter_name.as_ref().map(|v|v.as_ref()),"product_listing_id":v.product_listing_id,"origin_event_id":v.origin_event_id,"feedback":v.feedback,"created":retry::time_value(v.created)?,"updated":retry::time_value(v.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        let name: Option<String> = retry::field(&v, "search_filter_name")?;
        if name.as_ref().is_some_and(|s| s.len() > 255) {
            return Err(static_error("invalid filter name"));
        }
        Ok(Self {
            search_filter_match: SearchFilterMatchView {
                user_id: retry::field(&v, "user_id")?,
                search_filter_id: retry::field(&v, "search_filter_id")?,
                search_filter_name: name.map(Into::into),
                product_listing_id: retry::field(&v, "product_listing_id")?,
                origin_event_id: retry::field(&v, "origin_event_id")?,
                feedback: retry::field(&v, "feedback")?,
                created: retry::timestamp(&v, "created")?,
                updated: retry::timestamp(&v, "updated")?,
            },
        })
    }
}

pub(crate) fn update_intent(command: &crate::use_cases::UpdateOwnedSearchFilterCommand) -> Value {
    use crate::use_cases::ProductListingSearchPatch;
    use application::patch_field::PatchField;
    let ProductListingSearchPatch {
        geography,
        language,
        currency,
        product_listing_query,
        enhanced_search_description,
        listing_source_id_query,
        exclude_listing_source_id_query,
        auction_id_query,
        price_query,
        availability_query,
        created_query,
        updated_query,
        lot_bidding_opens_query,
        lot_scheduled_closes_query,
    } = &command.search;
    let mut search = serde_json::Map::new();
    macro_rules! leaf {($field:ident)=>{{let value=match $field {PatchField::Unchanged=>json!({"kind":"UNCHANGED"}),PatchField::Clear=>json!({"kind":"CLEAR"}),PatchField::Set(value)=>{let mut candidate=ProductListingSearch::new(Language::En,Currency::Eur);candidate.$field=value.clone().into();let intent=crate::create_receipt::search_value(&candidate);json!({"kind":"SET","value":intent[stringify!($field)].clone()})}};search.insert(stringify!($field).into(),value);}};}
    leaf!(geography);
    leaf!(language);
    leaf!(currency);
    leaf!(product_listing_query);
    leaf!(enhanced_search_description);
    leaf!(listing_source_id_query);
    leaf!(exclude_listing_source_id_query);
    leaf!(auction_id_query);
    leaf!(price_query);
    leaf!(availability_query);
    leaf!(created_query);
    leaf!(updated_query);
    leaf!(lot_bidding_opens_query);
    leaf!(lot_scheduled_closes_query);
    json!({"user_id":command.user_id,"search_filter_id":command.search_filter_id,"name":retry::patch(&command.name,|v|json!(v.as_ref())),"notifications":retry::patch(&command.notifications,|v|json!(v)),"state":retry::patch(&command.state,|v|json!(v.as_str())),"search":search})
}
