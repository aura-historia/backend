//! Transport-independent CDC route policy, validation and compact job construction.
use std::collections::BTreeMap;
use std::fmt::Display;

use aura_historia_jobs::jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey,
    ProductListingEventJob, ProductListingRawRevisionJob, SearchFilterChangedJob,
    SearchFilterMatchCreatedJob, SearchFilterOperation, WorkerQueue,
};
use domain_primitives::event_id::EventId;
use localization::Language;
use notification_core::notification_delivery_id::NotificationDeliveryId;
use product_listing_core::product_listing_raw_id::{
    ProductListingRawRevisionId, ProductListingRawStreamId,
};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing_auction::LotNumber,
    product_listing_id::{ProductListingId, ProductListingKey},
    source_listing_id::SourceListingId,
    title::Title,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::warn;
use url::Url;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Insert,
    Update,
    Delete,
}

/// Source operation syntax and the job's operation type remain owned by each runtime.
pub trait RouteOperation: Copy + Display {
    type ParseError: Display;
    fn parse(value: &str) -> Result<Self, Self::ParseError>;
    fn kind(self) -> Operation;
}

impl RouteOperation for SearchFilterOperation {
    type ParseError = &'static str;

    fn parse(value: &str) -> Result<Self, Self::ParseError> {
        match value {
            "insert" => Ok(Self::Insert),
            "update" => Ok(Self::Update),
            "delete" => Ok(Self::Delete),
            _ => Err("unsupported CDC operation"),
        }
    }

    fn kind(self) -> Operation {
        match self {
            Self::Insert => Operation::Insert,
            Self::Update => Operation::Update,
            Self::Delete => Operation::Delete,
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(bound(deserialize = "O: RouteOperation"))]
pub struct CdcChange<O: RouteOperation> {
    #[serde(default, alias = "table_schema")]
    pub schema: Option<String>,
    #[serde(alias = "relation")]
    pub table: String,
    #[serde(
        alias = "op",
        alias = "action",
        deserialize_with = "deserialize_operation"
    )]
    pub operation: O,
    #[serde(default, alias = "keys")]
    pub primary_key: BTreeMap<String, Value>,
    #[serde(default, alias = "new", alias = "new_record")]
    pub record: Option<Value>,
    #[serde(default, rename = "old", alias = "old_record", alias = "previous")]
    pub old_record: Option<Value>,
    #[serde(default, alias = "changed")]
    pub changed_columns: Vec<String>,
    #[serde(default)]
    pub commit_lsn: Option<String>,
    #[serde(default)]
    pub commit_timestamp: Option<String>,
}

fn deserialize_operation<'de, D: Deserializer<'de>, O: RouteOperation>(
    deserializer: D,
) -> Result<O, D::Error> {
    let value = String::deserialize(deserializer)?;
    O::parse(&value).map_err(serde::de::Error::custom)
}
pub fn route_change<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let table = CdcTable::from(change.table.as_str());
    match (table, change.operation.kind()) {
        (CdcTable::ProductListingEvents, Operation::Insert) => product_event_jobs(change),
        (CdcTable::ProductListingEvents, _) => Ok(Vec::new()),
        (CdcTable::ProductListingRawRevisions, Operation::Insert) => {
            product_listing_raw_revision_job(change)
        }
        (CdcTable::ProductListingRawRevisions, _) => Ok(Vec::new()),
        (CdcTable::SearchFilters, _) => search_filter_changed_job(change, change.operation),
        (CdcTable::SearchFilterMatches, Operation::Insert) => {
            search_filter_match_created_job(change)
        }
        (CdcTable::SearchFilterMatches, _) => Ok(Vec::new()),
        (CdcTable::Users, _) => Ok(Vec::new()),
        (CdcTable::NotificationDeliveries, Operation::Insert) => {
            notification_delivery_created_job(change)
        }
        (CdcTable::NotificationDeliveries, _) => Ok(Vec::new()),
        (CdcTable::Unknown(_), _) => {
            warn!(operation = %change.operation, "ignoring unregistered CDC table");
            Ok(Vec::new())
        }
        (CdcTable::ProductListings | CdcTable::ProductListingWatchlist, _) => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdcTable {
    ProductListingEvents,
    ProductListingRawRevisions,
    ProductListings,
    SearchFilters,
    SearchFilterMatches,
    Users,
    ProductListingWatchlist,
    NotificationDeliveries,
    Unknown(String),
}

impl From<&str> for CdcTable {
    fn from(value: &str) -> Self {
        match value {
            "product_listing_events" => Self::ProductListingEvents,
            "product_listing_raw_revisions" => Self::ProductListingRawRevisions,
            "product_listings" => Self::ProductListings,
            "search_filters" => Self::SearchFilters,
            "search_filter_matches" => Self::SearchFilterMatches,
            "users" => Self::Users,
            "product_listing_watchlist" => Self::ProductListingWatchlist,
            "notification_deliveries" => Self::NotificationDeliveries,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

const PRODUCT_LISTING_EVENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingRoutingEventKind {
    Discovered,
    Changed,
    Embedded,
    TranslatedTitles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProductListingEventRoutingFacts {
    event_id: EventId,
    product_listing_id: ProductListingId,
    event_kind: ProductListingRoutingEventKind,
    has_main_price_change: bool,
    has_availability_change: bool,
    has_image_change: bool,
}

fn product_event_routing_facts<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<ProductListingEventRoutingFacts, CdcRouteError> {
    let row = required_row(change)?;
    let event_id_value = required_product_event_string(row, "event_id")?;
    let event_id = parse_canonical_storage_object_id(
        &event_id_value,
        || CdcRouteError::InvalidEventId,
        || CdcRouteError::InvalidEventId,
    )?;
    let product_listing_id_value = required_product_event_string(row, "product_listing_id")?;
    let product_listing_id = parse_canonical_storage_object_id(
        &product_listing_id_value,
        || CdcRouteError::InvalidProductListingId,
        || CdcRouteError::InvalidProductListingId,
    )?;
    let event_type = required_product_event_string(row, "event_type")?;
    let event_group = required_product_event_string(row, "event_group")?;
    let schema_version = required_integer(row, "event_type_schema_version")?;
    let payload = row
        .get("payload")
        .ok_or(CdcRouteError::MissingColumn("payload"))?;

    if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
        return Err(CdcRouteError::UnsupportedProductListingEventSchemaVersion { schema_version });
    }

    let event_kind = match (event_type.as_str(), event_group.as_str()) {
        ("PRODUCT_LISTING_DISCOVERED", "DOMAIN") => {
            validate_discovered_payload(payload)?;
            ProductListingRoutingEventKind::Discovered
        }
        ("PRODUCT_LISTING_CHANGED", "DOMAIN") => {
            let (has_main_price_change, has_availability_change, has_image_change) =
                validate_changed_payload(payload)?;
            return Ok(ProductListingEventRoutingFacts {
                event_id,
                product_listing_id,
                event_kind: ProductListingRoutingEventKind::Changed,
                has_main_price_change,
                has_availability_change,
                has_image_change,
            });
        }
        ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => {
            validate_embedded_payload(payload)?;
            ProductListingRoutingEventKind::Embedded
        }
        ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT") => {
            validate_translated_payload(payload)?;
            ProductListingRoutingEventKind::TranslatedTitles
        }
        ("PRODUCT_LISTING_DISCOVERED" | "PRODUCT_LISTING_CHANGED", _)
        | ("ENRICHMENT_EMBEDDED" | "ENRICHMENT_TRANSLATED_TITLES", _) => {
            return Err(CdcRouteError::UnsupportedProductListingEvent {
                event_type,
                event_group,
            });
        }
        _ => {
            return Err(CdcRouteError::UnsupportedProductListingEvent {
                event_type,
                event_group,
            });
        }
    };

    Ok(ProductListingEventRoutingFacts {
        event_id,
        product_listing_id,
        event_kind,
        has_main_price_change: false,
        has_availability_change: false,
        has_image_change: false,
    })
}

pub fn product_event_jobs<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let facts = product_event_routing_facts(change)?;
    let base_job = ProductListingEventJob {
        event_id: facts.event_id,
        product_listing_id: facts.product_listing_id,
    };
    let idempotency_key = IdempotencyKey::new(format!("product-event:{}", facts.event_id));
    let ordering_key = OrderingKey::new(format!("product:{}", facts.product_listing_id));

    let mut jobs = vec![domain_job(
        WorkerQueue::ProductListingOpenSearch,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    )];
    jobs.push(domain_job(
        WorkerQueue::SearchFilterPercolator,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    ));

    match facts.event_kind {
        ProductListingRoutingEventKind::Discovered => {
            for target_queue in [
                WorkerQueue::ProductListingContentAssessment,
                WorkerQueue::ProductListingEmbed,
                WorkerQueue::ProductListingTranslate,
            ] {
                jobs.push(domain_job(
                    target_queue,
                    idempotency_key.clone(),
                    ordering_key.clone(),
                    DomainJobPayload::ProductListingEvent(base_job.clone()),
                ));
            }
        }
        ProductListingRoutingEventKind::Changed => {
            if facts.has_main_price_change || facts.has_availability_change {
                jobs.push(domain_job(
                    WorkerQueue::WatchlistNotification,
                    idempotency_key.clone(),
                    ordering_key.clone(),
                    DomainJobPayload::ProductListingEvent(base_job.clone()),
                ));
            }
            if facts.has_image_change {
                jobs.push(domain_job(
                    WorkerQueue::ProductListingEmbed,
                    idempotency_key,
                    ordering_key,
                    DomainJobPayload::ProductListingEvent(base_job),
                ));
            }
        }
        ProductListingRoutingEventKind::Embedded
        | ProductListingRoutingEventKind::TranslatedTitles => {}
    }

    Ok(jobs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingLifecycleRoutingChange {
    Withdrawn,
    Restored,
}

fn validate_discovered_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(
        object,
        &[
            "listingSourceId",
            "sourceListingId",
            "title",
            "description",
            "pricing",
            "availability",
            "url",
            "imageCount",
            "auction",
        ],
        "payload",
    )?;
    let source_listing_id =
        validate_source_listing_id(&require_string(object, "sourceListingId")?)?;
    validate_listing_source_id(
        &require_string(object, "listingSourceId")?,
        source_listing_id,
    )?;
    require_nullable_localized(object, "title", canonical_title)?;
    require_nullable_localized(object, "description", canonical_description)?;

    let pricing = require_object_field(object, "pricing")?;
    require_exact_keys(
        pricing,
        &["price", "priceEstimateMin", "priceEstimateMax"],
        "pricing",
    )?;
    validate_product_listing_price(
        pricing
            .get("price")
            .ok_or(CdcRouteError::MissingColumn("price"))?,
    )?;
    for field in ["priceEstimateMin", "priceEstimateMax"] {
        validate_price(
            pricing
                .get(field)
                .ok_or(CdcRouteError::MissingColumn(field))?,
        )?;
    }
    validate_nullable_availability(require_value(object, "availability")?, "availability")?;
    validate_canonical_url(&require_string(object, "url")?, "url")?;
    require_u64(object, "imageCount")?;
    validate_auction(require_value(object, "auction")?, "auction")?;
    Ok(())
}

fn validate_changed_payload(value: &Value) -> Result<(bool, bool, bool), CdcRouteError> {
    let object = required_payload_object(value)?;
    if object.is_empty() {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    require_known_keys(
        object,
        &[
            "pricing",
            "availability",
            "url",
            "images",
            "auction",
            "lifecycle",
            "saleObservation",
        ],
        "payload",
    )?;

    let mut has_main_price_change = false;
    let mut has_availability_change = false;
    let mut has_image_change = false;
    let mut availability_change = None;
    let mut lifecycle_change = None;
    for (field, value) in object {
        match field.as_str() {
            "pricing" => {
                let pricing = required_object(value)?;
                if pricing.is_empty() {
                    return Err(CdcRouteError::InvalidProductListingEventPayload);
                }
                require_known_keys(
                    pricing,
                    &["price", "priceEstimateMin", "priceEstimateMax"],
                    "pricing",
                )?;
                for (pricing_field, value) in pricing {
                    match pricing_field.as_str() {
                        "price" => {
                            validate_product_listing_price_change(value)?;
                            has_main_price_change = true;
                        }
                        "priceEstimateMin" | "priceEstimateMax" => {
                            validate_price_change(value)?;
                        }
                        _ => return Err(CdcRouteError::InvalidProductListingEventPayload),
                    }
                }
            }
            "availability" => {
                availability_change = Some(validate_code_change(value)?);
                has_availability_change = true;
            }
            "url" => validate_string_change(value)?,
            "images" => {
                let images = required_object(value)?;
                require_exact_keys(images, &["previousCount", "currentCount"], "images")?;
                require_u64(images, "previousCount")?;
                require_u64(images, "currentCount")?;
                has_image_change = true;
            }
            "auction" => validate_auction_change(value)?,
            "lifecycle" => lifecycle_change = Some(validate_lifecycle(value)?),
            "saleObservation" => validate_sale_observation(value)?,
            _ => return Err(CdcRouteError::InvalidProductListingEventPayload),
        }
    }

    match (lifecycle_change, availability_change) {
        (Some(ProductListingLifecycleRoutingChange::Withdrawn), Some((_, Some(_)))) => {
            return Err(CdcRouteError::InconsistentProductListingEvent {
                rule: "withdrawal with current availability".to_owned(),
            });
        }
        (Some(ProductListingLifecycleRoutingChange::Restored), Some((Some(_), _))) => {
            return Err(CdcRouteError::InconsistentProductListingEvent {
                rule: "restoration with previous availability".to_owned(),
            });
        }
        _ => {}
    }

    Ok((
        has_main_price_change,
        has_availability_change,
        has_image_change,
    ))
}

fn validate_product_listing_price_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "price change")?;
    let previous = require_value(change, "previous")?;
    let current = require_value(change, "current")?;
    validate_product_listing_price(previous)?;
    validate_product_listing_price(current)?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_product_listing_price(value: &Value) -> Result<(), CdcRouteError> {
    if value.is_null() {
        return Ok(());
    }

    let object = required_object(value)?;
    match require_string(object, "type")?.as_str() {
        "MONETARY" => {
            require_exact_keys(
                object,
                &["type", "amount", "currency"],
                "product listing price",
            )?;
            require_u64(object, "amount")?;
            let currency = require_string(object, "currency")?;
            let parsed = money::Currency::from_code(currency.as_str())
                .ok_or_else(|| invalid_product_listing_field("price.currency"))?;
            if parsed.as_str() != currency {
                return Err(noncanonical_product_listing_field("price.currency"));
            }
            Ok(())
        }
        "ON_REQUEST" => {
            require_exact_keys(object, &["type"], "product listing price")?;
            Ok(())
        }
        _ => Err(CdcRouteError::InvalidProductListingEventPayload),
    }
}

fn validate_price_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "price change")?;
    let previous = require_value(change, "previous")?;
    let current = require_value(change, "current")?;
    validate_price(previous)?;
    validate_price(current)?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_price(value: &Value) -> Result<(), CdcRouteError> {
    if value.is_null() {
        return Ok(());
    }
    let object = required_object(value)?;
    require_exact_keys(object, &["amount", "currency"], "price")?;
    require_u64(object, "amount")?;
    let currency = require_string(object, "currency")?;
    let parsed = money::Currency::from_code(currency.as_str())
        .ok_or_else(|| invalid_product_listing_field("price.currency"))?;
    if parsed.as_str() != currency {
        return Err(noncanonical_product_listing_field("price.currency"));
    }
    Ok(())
}

fn validate_code_change(
    value: &Value,
) -> Result<(Option<ListingAvailability>, Option<ListingAvailability>), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "availability change")?;
    let previous = validate_nullable_availability(
        require_value(change, "previous")?,
        "availability.previous",
    )?;
    let current =
        validate_nullable_availability(require_value(change, "current")?, "availability.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok((previous, current))
}

fn validate_string_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "url change")?;
    let previous = validate_canonical_url(&require_string(change, "previous")?, "url.previous")?;
    let current = validate_canonical_url(&require_string(change, "current")?, "url.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_auction_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "auction change")?;
    let previous = validate_auction(require_value(change, "previous")?, "auction.previous")?;
    let current = validate_auction(require_value(change, "current")?, "auction.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_auction(value: &Value, field: &str) -> Result<Option<Value>, CdcRouteError> {
    if value.is_null() {
        return Ok(None);
    }
    let object = required_object(value)?;
    require_exact_keys(
        object,
        &[
            "auctionId",
            "lotNumber",
            "cataloguePosition",
            "lotBiddingOpensAt",
            "lotScheduledClosesAt",
            "lotReportedClosedAt",
        ],
        field,
    )?;

    match require_value(object, "auctionId")? {
        Value::Null => {}
        Value::String(value) => {
            let auction_id = parse_canonical_storage_uuid(
                value,
                || invalid_product_listing_field(format!("{field}.auctionId")),
                || noncanonical_product_listing_field(format!("{field}.auctionId")),
            )?;
            if auction_id.get_version_num() != 7 {
                return Err(invalid_product_listing_field(format!("{field}.auctionId")));
            }
        }
        _ => return Err(invalid_product_listing_field(format!("{field}.auctionId"))),
    }
    match require_value(object, "lotNumber")? {
        Value::Null => {}
        Value::String(value) => {
            let parsed = LotNumber::try_from(value.as_str())
                .map_err(|_| invalid_product_listing_field(format!("{field}.lotNumber")))?;
            if parsed.as_str() != value {
                return Err(noncanonical_product_listing_field(format!(
                    "{field}.lotNumber"
                )));
            }
        }
        _ => return Err(invalid_product_listing_field(format!("{field}.lotNumber"))),
    }
    match require_value(object, "cataloguePosition")? {
        Value::Null => {}
        value => {
            let position = value.as_u64().ok_or_else(|| {
                invalid_product_listing_field(format!("{field}.cataloguePosition"))
            })?;
            if position == 0 || position > u64::from(u32::MAX) {
                return Err(invalid_product_listing_field(format!(
                    "{field}.cataloguePosition"
                )));
            }
        }
    }
    let bidding_opens = parse_nullable_timestamp(
        require_value(object, "lotBiddingOpensAt")?,
        format!("{field}.lotBiddingOpensAt").as_str(),
    )?;
    let scheduled_closes = parse_nullable_timestamp(
        require_value(object, "lotScheduledClosesAt")?,
        format!("{field}.lotScheduledClosesAt").as_str(),
    )?;
    let _reported_closed_at = parse_nullable_timestamp(
        require_value(object, "lotReportedClosedAt")?,
        format!("{field}.lotReportedClosedAt").as_str(),
    )?;
    if bidding_opens
        .zip(scheduled_closes)
        .is_some_and(|(open, close)| open > close)
    {
        return Err(CdcRouteError::InconsistentProductListingEvent {
            rule: format!("{field} bidding opens after scheduled close"),
        });
    }

    Ok(Some(value.clone()))
}

fn validate_lifecycle(
    value: &Value,
) -> Result<ProductListingLifecycleRoutingChange, CdcRouteError> {
    let object = required_object(value)?;
    let transition = require_string(object, "transition")?;
    match transition.as_str() {
        "WITHDRAWN" => {
            require_exact_keys(object, &["transition", "previousAvailability"], "lifecycle")?;
            validate_nullable_availability(
                require_value(object, "previousAvailability")?,
                "lifecycle.previousAvailability",
            )?;
            Ok(ProductListingLifecycleRoutingChange::Withdrawn)
        }
        "RESTORED" => {
            require_exact_keys(object, &["transition"], "lifecycle")?;
            Ok(ProductListingLifecycleRoutingChange::Restored)
        }
        _ => Err(invalid_product_listing_field("lifecycle.transition")),
    }
}

fn validate_sale_observation(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_object(value)?;
    require_exact_keys(object, &["transition", "observation"], "saleObservation")?;
    let transition = require_string(object, "transition")?;
    if !matches!(transition.as_str(), "OBSERVED" | "RETRACTED") {
        return Err(invalid_product_listing_field("saleObservation.transition"));
    }
    let observation = require_object_field(object, "observation")?;
    require_exact_keys(
        observation,
        &["observedAt", "fxRateId"],
        "saleObservation.observation",
    )?;
    parse_canonical_timestamp(
        &require_string(observation, "observedAt")?,
        "saleObservation.observedAt",
    )?;
    let fx_rate_id = require_string(observation, "fxRateId")?;
    let _: fxrate_core::FxRateId = parse_canonical_storage_object_id(
        &fx_rate_id,
        || invalid_product_listing_field("saleObservation.fxRateId"),
        || noncanonical_product_listing_field("saleObservation.fxRateId"),
    )?;
    Ok(())
}

fn validate_embedded_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(object, &["sourceEventId"], "payload")?;
    validate_canonical_event_id(&require_string(object, "sourceEventId")?)
}

fn validate_translated_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(
        object,
        &["sourceEventId", "sourceLanguage", "targetLanguages"],
        "payload",
    )?;
    validate_canonical_event_id(&require_string(object, "sourceEventId")?)?;
    let source_language = require_string(object, "sourceLanguage")?;
    validate_language(&source_language, "sourceLanguage")?;
    let target_languages = object
        .get("targetLanguages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_product_listing_field("targetLanguages"))?;
    if target_languages.is_empty() {
        return Err(invalid_product_listing_field("targetLanguages"));
    }
    for language in target_languages {
        let language = language
            .as_str()
            .ok_or_else(|| invalid_product_listing_field("targetLanguages"))?;
        validate_language(language, "targetLanguages")?;
    }
    Ok(())
}

fn required_payload_object(value: &Value) -> Result<&Map<String, Value>, CdcRouteError> {
    required_object(value)
}

fn required_object(value: &Value) -> Result<&Map<String, Value>, CdcRouteError> {
    value
        .as_object()
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), CdcRouteError> {
    for field in expected {
        if !object.contains_key(*field) {
            return Err(CdcRouteError::MissingProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    for field in object.keys() {
        if !expected.contains(&field.as_str()) {
            return Err(CdcRouteError::UnknownProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    Ok(())
}

fn require_known_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), CdcRouteError> {
    for field in object.keys() {
        if !expected.contains(&field.as_str()) {
            return Err(CdcRouteError::UnknownProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    Ok(())
}

fn require_value<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, CdcRouteError> {
    object.get(field).ok_or(CdcRouteError::MissingColumn(field))
}

fn require_object_field<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, CdcRouteError> {
    required_object(require_value(object, field)?)
}

fn require_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, CdcRouteError> {
    require_value(object, field)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_u64(object: &Map<String, Value>, field: &'static str) -> Result<u64, CdcRouteError> {
    require_value(object, field)?
        .as_u64()
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_nullable_localized(
    object: &Map<String, Value>,
    field: &'static str,
    canonicalize: fn(&str) -> String,
) -> Result<(), CdcRouteError> {
    let value = require_value(object, field)?;
    if value.is_null() {
        return Ok(());
    }
    let localized = required_object(value)?;
    require_exact_keys(localized, &["language", "text"], field)?;
    let language = require_string(localized, "language")?;
    validate_language(&language, &format!("{field}.language"))?;
    let text = require_string(localized, "text")?;
    if canonicalize(text.as_str()) != text {
        return Err(noncanonical_product_listing_field(format!("{field}.text")));
    }
    Ok(())
}

fn canonical_title(value: &str) -> String {
    Title::from(value).to_string()
}

fn canonical_description(value: &str) -> String {
    Description::from(value).to_string()
}

fn validate_listing_source_id(
    value: &str,
    source_listing_id: SourceListingId,
) -> Result<(), CdcRouteError> {
    let id = parse_canonical_storage_uuid(
        value,
        || invalid_product_listing_field("listingSourceId"),
        || noncanonical_product_listing_field("listingSourceId"),
    )?;
    let listing_source_id = id
        .try_into()
        .map_err(|_| invalid_product_listing_field("listingSourceId"))?;
    let _ = ProductListingKey::new(listing_source_id, source_listing_id);
    Ok(())
}

fn validate_source_listing_id(value: &str) -> Result<SourceListingId, CdcRouteError> {
    let id = SourceListingId::try_from(value)
        .map_err(|_| invalid_product_listing_field("sourceListingId"))?;
    if id.as_ref() != value {
        return Err(noncanonical_product_listing_field("sourceListingId"));
    }
    Ok(id)
}

fn validate_canonical_event_id(value: &str) -> Result<(), CdcRouteError> {
    let _: EventId = parse_canonical_storage_object_id(
        value,
        || invalid_product_listing_field("sourceEventId"),
        || noncanonical_product_listing_field("sourceEventId"),
    )?;
    Ok(())
}

fn parse_canonical_storage_uuid<F, N>(
    value: &str,
    invalid: F,
    noncanonical: N,
) -> Result<Uuid, CdcRouteError>
where
    F: Fn() -> CdcRouteError,
    N: Fn() -> CdcRouteError,
{
    let id = Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.to_string() != value {
        return Err(noncanonical());
    }
    Ok(id)
}

fn parse_canonical_storage_object_id<T, F, N>(
    value: &str,
    invalid: F,
    noncanonical: N,
) -> Result<T, CdcRouteError>
where
    T: TryFrom<Uuid>,
    F: Fn() -> CdcRouteError,
    N: Fn() -> CdcRouteError,
{
    parse_canonical_storage_uuid(value, &invalid, noncanonical)?
        .try_into()
        .map_err(|_| invalid())
}

fn validate_language(value: &str, field: &str) -> Result<(), CdcRouteError> {
    Language::from_code(value)
        .map(|_| ())
        .ok_or_else(|| invalid_product_listing_field(field))
}

fn validate_nullable_availability(
    value: &Value,
    field: &str,
) -> Result<Option<ListingAvailability>, CdcRouteError> {
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| invalid_product_listing_field(field))?;
    let availability = ListingAvailability::from_code(value)
        .ok_or_else(|| invalid_product_listing_field(field))?;
    if availability.as_str() != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(Some(availability))
}

fn validate_canonical_url(value: &str, field: &str) -> Result<Url, CdcRouteError> {
    let url = Url::parse(value).map_err(|_| invalid_product_listing_field(field))?;
    if url.as_str() != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(url)
}

fn parse_nullable_timestamp(
    value: &Value,
    field: &str,
) -> Result<Option<OffsetDateTime>, CdcRouteError> {
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| invalid_product_listing_field(field))?;
    parse_canonical_timestamp(value, field).map(Some)
}

fn parse_canonical_timestamp(value: &str, field: &str) -> Result<OffsetDateTime, CdcRouteError> {
    let timestamp =
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| invalid_product_listing_field(field))?;
    let canonical = timestamp
        .format(&Rfc3339)
        .map_err(|_| invalid_product_listing_field(field))?;
    if canonical != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(timestamp)
}

fn invalid_product_listing_field(field: impl Into<String>) -> CdcRouteError {
    CdcRouteError::InvalidProductListingEventField {
        field: field.into(),
    }
}

fn noncanonical_product_listing_field(field: impl Into<String>) -> CdcRouteError {
    CdcRouteError::NonCanonicalProductListingEventField {
        field: field.into(),
    }
}

fn required_product_event_string(
    row: &Value,
    field: &'static str,
) -> Result<String, CdcRouteError> {
    row.as_object()
        .ok_or(CdcRouteError::MissingRow)?
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(CdcRouteError::MissingColumn(field))
}

pub fn search_filter_changed_job<O: RouteOperation>(
    change: &CdcChange<O>,
    operation: O,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let row = row_for_operation(change)?;
    let user_search_filter_id: UserSearchFilterId = parse_canonical_storage_object_id(
        &required_string(row, "user_search_filter_id")?,
        || CdcRouteError::InvalidObjectId("user_search_filter_id"),
        || CdcRouteError::InvalidObjectId("user_search_filter_id"),
    )?;
    let user_id: UserId = parse_canonical_storage_object_id(
        &required_string(row, "user_id")?,
        || CdcRouteError::InvalidObjectId("user_id"),
        || CdcRouteError::InvalidObjectId("user_id"),
    )?;
    let version = required_integer(row, "version")?;
    if version <= 0 {
        return Err(CdcRouteError::InvalidSearchFilterVersion);
    }

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterOpenSearch,
        IdempotencyKey::new(format!(
            "search-filter:{user_search_filter_id}:{version}:{operation}"
        )),
        OrderingKey::new(format!("search-filter:{user_search_filter_id}")),
        DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
            user_id,
            user_search_filter_id,
            version,
            operation,
        }),
    )])
}

pub fn search_filter_match_created_job<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let row = required_row(change)?;
    let user_id: UserId = parse_canonical_storage_object_id(
        &required_string(row, "user_id")?,
        || CdcRouteError::InvalidObjectId("user_id"),
        || CdcRouteError::InvalidObjectId("user_id"),
    )?;
    let user_search_filter_id: UserSearchFilterId = parse_canonical_storage_object_id(
        &required_string(row, "user_search_filter_id")?,
        || CdcRouteError::InvalidObjectId("user_search_filter_id"),
        || CdcRouteError::InvalidObjectId("user_search_filter_id"),
    )?;
    let product_listing_id: ProductListingId = parse_canonical_storage_object_id(
        &required_string(row, "product_listing_id")?,
        || CdcRouteError::InvalidObjectId("product_listing_id"),
        || CdcRouteError::InvalidObjectId("product_listing_id"),
    )?;
    let origin_event_id: EventId = parse_canonical_storage_object_id(
        &required_string(row, "origin_event_id")?,
        || CdcRouteError::InvalidObjectId("origin_event_id"),
        || CdcRouteError::InvalidObjectId("origin_event_id"),
    )?;

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterMatchNotification,
        IdempotencyKey::new(format!(
            "search-filter-match:{user_id}:{user_search_filter_id}:{product_listing_id}:{origin_event_id}"
        )),
        OrderingKey::new(format!("user:{user_id}")),
        DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id,
        }),
    )])
}

pub fn product_listing_raw_revision_job<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let row = required_row(change)?;
    let stream_id_value = required_string(row, "product_listing_raw_stream_id")?;
    let stream_id: ProductListingRawStreamId = parse_canonical_storage_object_id(
        &stream_id_value,
        || CdcRouteError::InvalidProductListingRawStreamId,
        || CdcRouteError::InvalidProductListingRawStreamId,
    )?;
    let revision_id_value = required_string(row, "product_listing_raw_revision_id")?;
    let revision_id: ProductListingRawRevisionId = parse_canonical_storage_object_id(
        &revision_id_value,
        || CdcRouteError::InvalidProductListingRawRevisionId,
        || CdcRouteError::InvalidProductListingRawRevisionId,
    )?;
    let revision = required_integer(row, "revision")?;
    let revision =
        u64::try_from(revision).map_err(|_| CdcRouteError::InvalidProductListingRawRevision)?;
    if revision == 0 {
        return Err(CdcRouteError::InvalidProductListingRawRevision);
    }

    Ok(vec![domain_job(
        WorkerQueue::ProductListingRawNormalization,
        IdempotencyKey::new(format!("product-listing-raw-revision:{revision_id}")),
        OrderingKey::new(format!("product-listing-raw-stream:{stream_id}")),
        DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
            product_listing_raw_stream_id: stream_id,
            product_listing_raw_revision_id: revision_id,
            revision,
        }),
    )])
}

pub fn notification_delivery_created_job<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<Vec<DomainJob<O>>, CdcRouteError> {
    let row = required_row(change)?;
    let notification_delivery_id: NotificationDeliveryId = parse_canonical_storage_object_id(
        &required_string(row, "notification_delivery_id")?,
        || CdcRouteError::InvalidObjectId("notification_delivery_id"),
        || CdcRouteError::InvalidObjectId("notification_delivery_id"),
    )?;

    Ok(vec![domain_job(
        WorkerQueue::NotificationDelivery,
        IdempotencyKey::new(format!("notification-delivery:{notification_delivery_id}")),
        OrderingKey::new(format!("notification-delivery:{notification_delivery_id}")),
        DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id,
        }),
    )])
}

fn domain_job<O: RouteOperation>(
    target_queue: WorkerQueue,
    idempotency_key: IdempotencyKey,
    ordering_key: OrderingKey,
    payload: DomainJobPayload<O>,
) -> DomainJob<O> {
    DomainJob {
        target_queue,
        idempotency_key,
        ordering_key,
        payload,
    }
}

pub fn row_for_operation<O: RouteOperation>(
    change: &CdcChange<O>,
) -> Result<&Value, CdcRouteError> {
    match change.operation.kind() {
        Operation::Delete => change.old_record.as_ref().ok_or(CdcRouteError::MissingRow),
        Operation::Insert | Operation::Update => required_row(change),
    }
}

pub fn required_row<O: RouteOperation>(change: &CdcChange<O>) -> Result<&Value, CdcRouteError> {
    change.record.as_ref().ok_or(CdcRouteError::MissingRow)
}

fn required_string(row: &Value, field: &'static str) -> Result<String, CdcRouteError> {
    string_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

fn string_field(row: &Value, field: &str) -> Option<String> {
    let value = row.get(field)?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn integer_field(row: &Value, field: &str) -> Option<i64> {
    match row.get(field)? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => canonical_decimal_i64(value),
        _ => None,
    }
}

pub fn canonical_decimal_i64(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
    {
        return None;
    }
    value.parse().ok()
}

fn required_integer(row: &Value, field: &'static str) -> Result<i64, CdcRouteError> {
    integer_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcRouteError {
    #[error("CDC source record violates the supported contract: {0}")]
    InvalidSourceContract(&'static str),
    #[error("CDC table is not configured for this worker: {0}")]
    UnsupportedTableForWorker(String),
    #[error("CDC change has unsupported product listing event {event_type} in group {event_group}")]
    UnsupportedProductListingEvent {
        event_type: String,
        event_group: String,
    },
    #[error("CDC change has unsupported product listing event schema version {schema_version}")]
    UnsupportedProductListingEventSchemaVersion { schema_version: i64 },
    #[error("CDC change has an invalid product listing event ID")]
    InvalidEventId,
    #[error("CDC change has an invalid product listing ID")]
    InvalidProductListingId,
    #[error("CDC change has an invalid raw product listing stream ID")]
    InvalidProductListingRawStreamId,
    #[error("CDC change has an invalid raw product listing revision ID")]
    InvalidProductListingRawRevisionId,
    #[error("CDC change has an invalid raw product listing revision number")]
    InvalidProductListingRawRevision,
    #[error("CDC change has an invalid object ID in column {0}")]
    InvalidObjectId(&'static str),
    #[error("CDC change has an invalid positive search filter version")]
    InvalidSearchFilterVersion,
    #[error("CDC change has a missing ProductListing event field {field}")]
    MissingProductListingEventField { field: String },
    #[error("CDC change has an invalid ProductListing event field {field}")]
    InvalidProductListingEventField { field: String },
    #[error("CDC change has a noncanonical ProductListing event field {field}")]
    NonCanonicalProductListingEventField { field: String },
    #[error("CDC change has an unknown ProductListing event field {field}")]
    UnknownProductListingEventField { field: String },
    #[error("CDC change has an inconsistent ProductListing event: {rule}")]
    InconsistentProductListingEvent { rule: String },
    #[error("CDC change has a product listing event payload that is not an object")]
    InvalidProductListingEventPayload,
    #[error("CDC change missing row data")]
    MissingRow,
    #[error("CDC row missing required column {0}")]
    MissingColumn(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn change(table: &str, operation: &str, record: Value) -> CdcChange<SearchFilterOperation> {
        serde_json::from_value(json!({
            "table": table,
            "operation": operation,
            "record": record,
        }))
        .unwrap()
    }

    #[test]
    fn discovered_event_fanout_preserves_order_and_keys() {
        let jobs = route_change(&change(
            "product_listing_events",
            "insert",
            json!({
                "event_id": "01900000-0000-7000-8000-000000000004",
                "product_listing_id": "01900000-0000-7000-8000-000000000003",
                "event_type": "PRODUCT_LISTING_DISCOVERED",
                "event_group": "DOMAIN",
                "event_type_schema_version": 1,
                "payload": {
                    "listingSourceId": "01900000-0000-7000-8000-000000000001",
                    "sourceListingId": "fixture-source-id",
                    "title": null,
                    "description": null,
                    "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                    "availability": null,
                    "url": "https://example.test/product",
                    "imageCount": 0,
                    "auction": null
                }
            }),
        ))
        .unwrap();
        assert_eq!(
            jobs.iter().map(|job| job.target_queue).collect::<Vec<_>>(),
            vec![
                WorkerQueue::ProductListingOpenSearch,
                WorkerQueue::SearchFilterPercolator,
                WorkerQueue::ProductListingContentAssessment,
                WorkerQueue::ProductListingEmbed,
                WorkerQueue::ProductListingTranslate,
            ]
        );
        assert!(
            jobs.iter()
                .all(|job| job.idempotency_key == jobs[0].idempotency_key
                    && job.ordering_key == jobs[0].ordering_key)
        );
    }

    #[test]
    fn raw_revision_fanout_has_stable_keys_and_typed_metadata() {
        let jobs = route_change(&change(
            "product_listing_raw_revisions",
            "insert",
            json!({
                "product_listing_raw_stream_id": "01900000-0000-7000-8000-000000000001",
                "product_listing_raw_revision_id": "01900000-0000-7000-8000-000000000002",
                "revision": 3,
            }),
        ))
        .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].target_queue,
            WorkerQueue::ProductListingRawNormalization
        );
        assert_eq!(
            jobs[0].idempotency_key.as_str(),
            "product-listing-raw-revision:prr_01j0000000e008000000000002"
        );
        assert_eq!(
            jobs[0].ordering_key.as_str(),
            "product-listing-raw-stream:prs_01j0000000e008000000000001"
        );
        assert!(
            matches!(&jobs[0].payload, DomainJobPayload::ProductListingRawRevision(job) if job.revision == 3)
        );
    }

    #[test]
    fn search_filter_delete_uses_old_row_and_retains_operation() {
        let mut change = change("search_filters", "delete", json!({}));
        change.record = None;
        change.old_record = Some(json!({
            "user_id": "01900000-0000-7000-8000-000000000001",
            "user_search_filter_id": "01900000-0000-7000-8000-000000000002",
            "version": 2,
        }));
        let jobs = route_change(&change).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].target_queue, WorkerQueue::SearchFilterOpenSearch);
        assert!(jobs[0].idempotency_key.as_str().ends_with(":2:delete"));
        assert!(
            matches!(&jobs[0].payload, DomainJobPayload::SearchFilterChanged(job) if job.operation == SearchFilterOperation::Delete)
        );
        change.old_record = None;
        assert_eq!(
            route_change(&change).unwrap_err(),
            CdcRouteError::MissingRow
        );
    }

    #[test]
    fn policy_rejects_invalid_revision_and_strict_operation_aliases() {
        let invalid = change(
            "product_listing_raw_revisions",
            "insert",
            json!({
                "product_listing_raw_stream_id": "01900000-0000-7000-8000-000000000001",
                "product_listing_raw_revision_id": "01900000-0000-7000-8000-000000000002",
                "revision": 0,
            }),
        );
        assert_eq!(
            route_change(&invalid).unwrap_err(),
            CdcRouteError::InvalidProductListingRawRevision
        );
        assert!(
            serde_json::from_value::<CdcChange<SearchFilterOperation>>(json!({
                "table": "search_filters", "operation": "create"
            }))
            .is_err()
        );
    }
}
