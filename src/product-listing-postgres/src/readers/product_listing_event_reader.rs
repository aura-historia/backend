use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        ListingAvailabilityChanged, ListingSaleObservation, ListingSaleObservationRetracted,
        ListingSaleObserved, ProductListingAuction, ProductListingAuctionChanged,
        ProductListingCreated, ProductListingEventPayload, ProductListingImagesChanged,
        ProductListingPriceChanged, ProductListingPricing, ProductListingRestored,
        ProductListingUrlChanged, ProductListingWithdrawn,
    },
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::{
    ports::{
        ProductListingEventReadError, ProductListingEventReader, ProductListingEventReaderFactory,
    },
    use_cases::{ProductListingEvent, ProductListingEventLookup},
};
use serde_json::Value;
use sqlx::PgConnection;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEventReaderFactory;

struct SqlxProductListingEventReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingEventRow {
    product_listing_id: uuid::Uuid,
    event_id: uuid::Uuid,
    event_type: String,
    payload: Value,
    event_time: OffsetDateTime,
}

impl SqlxProductListingEventReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEventReaderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEventReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEventReader + 'tx {
        SqlxProductListingEventReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEventReader for SqlxProductListingEventReader<'_> {
    async fn find_domain_events(
        &mut self,
        lookup: &ProductListingEventLookup,
    ) -> Result<Option<Vec<ProductListingEvent>>, ProductListingEventReadError> {
        let product_listing_id = match lookup {
            ProductListingEventLookup::ById(product_listing_id) => {
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT product_listing_id FROM product_listings WHERE product_listing_id = $1",
                )
                .bind(uuid::Uuid::from(*product_listing_id))
                .fetch_optional(&mut *self.connection)
                .await
            }
            ProductListingEventLookup::BySlug {
                listing_source_slug_id,
                product_listing_slug_id,
            } => sqlx::query_scalar::<_, uuid::Uuid>(
                "SELECT p.product_listing_id FROM product_listings p JOIN listing_sources source ON source.listing_source_id = p.listing_source_id WHERE source.listing_source_slug_id = $1 AND p.product_listing_slug_id = $2",
            )
            .bind(listing_source_slug_id.as_ref())
            .bind(product_listing_slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await,
        }
        .map_err(|_| ProductListingEventReadError::ProductListingEventQueryFailed)?;

        let Some(product_listing_id) = product_listing_id else {
            return Ok(None);
        };

        let rows = sqlx::query_as::<_, ProductListingEventRow>(
            r#"
            SELECT event_id, product_listing_id, event_type, payload, event_time
            FROM product_listing_events
            WHERE product_listing_id = $1
              AND event_group = 'DOMAIN'
            ORDER BY event_time ASC, event_id ASC
            "#,
        )
        .bind(product_listing_id)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|_| ProductListingEventReadError::ProductListingEventQueryFailed)?;

        rows.into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

impl TryFrom<ProductListingEventRow> for ProductListingEvent {
    type Error = ProductListingEventReadError;

    fn try_from(row: ProductListingEventRow) -> Result<Self, Self::Error> {
        Ok(ProductListingEvent {
            product_listing_id: ProductListingId::from(row.product_listing_id),
            event_id: EventId::from(row.event_id),
            payload: parse_payload(&row.event_type, &row.payload)?,
            timestamp: row.event_time,
        })
    }
}

pub(crate) fn parse_payload(
    event_type: &str,
    payload: &Value,
) -> Result<ProductListingEventPayload, ProductListingEventReadError> {
    match event_type {
        "PRODUCT_LISTING_CREATED" => Ok(ProductListingEventPayload::Created(Box::new(
            ProductListingCreated {
                title: localized_title(payload.get("title"))?,
                description: localized_description(payload.get("description"))?,
                listing_source_id: listing_source_id(payload)?,
                source_listing_id: source_listing_id(payload)?,
                pricing: pricing(payload.get("pricing"))?,
                availability: availability(payload.get("availability"))?,
                url: url(string(payload, "url")?)?,
                images: images(payload.get("images"))?,
                auction: auction(payload.get("auction"))?,
            },
        ))),
        "PRODUCT_LISTING_AVAILABILITY_CHANGED" => Ok(
            ProductListingEventPayload::AvailabilityChanged(ListingAvailabilityChanged {
                previous: availability(payload.get("previousAvailability"))?,
                current: availability(payload.get("currentAvailability"))?,
            }),
        ),

        "PRODUCT_LISTING_PRICE_CHANGED" => Ok(ProductListingEventPayload::PriceChanged(
            ProductListingPriceChanged {
                old_pricing: pricing(payload.get("oldPricing"))?,
                new_pricing: pricing(payload.get("newPricing"))?,
            },
        )),
        "PRODUCT_LISTING_URL_CHANGED" => Ok(ProductListingEventPayload::UrlChanged(
            ProductListingUrlChanged {
                old_url: url(string(payload, "oldUrl")?)?,
                new_url: url(string(payload, "newUrl")?)?,
            },
        )),
        "PRODUCT_LISTING_IMAGES_CHANGED" => Ok(ProductListingEventPayload::ImagesChanged(
            Box::new(ProductListingImagesChanged {
                images: images(payload.get("images"))?,
            }),
        )),
        "PRODUCT_LISTING_AUCTION_CHANGED" => Ok(ProductListingEventPayload::AuctionChanged(
            ProductListingAuctionChanged {
                auction: auction(payload.get("auction"))?,
            },
        )),
        "PRODUCT_LISTING_WITHDRAWN" => Ok(ProductListingEventPayload::Withdrawn(
            ProductListingWithdrawn {
                previous_availability: availability(payload.get("previousAvailability"))?,
            },
        )),
        "PRODUCT_LISTING_RESTORED" => {
            Ok(ProductListingEventPayload::Restored(ProductListingRestored))
        }
        "PRODUCT_LISTING_SALE_OBSERVED" => Ok(ProductListingEventPayload::SaleObserved(
            ListingSaleObserved {
                observation: sale_observation(payload.get("observation"))?,
            },
        )),
        "PRODUCT_LISTING_SALE_OBSERVATION_RETRACTED" => Ok(
            ProductListingEventPayload::SaleObservationRetracted(ListingSaleObservationRetracted {
                observation: sale_observation(payload.get("observation"))?,
            }),
        ),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn object(
    value: Option<&Value>,
) -> Result<&serde_json::Map<String, Value>, ProductListingEventReadError> {
    value
        .and_then(Value::as_object)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, ProductListingEventReadError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn optional_string<'a>(
    value: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, ProductListingEventReadError> {
    match value.get(name) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(Value::Null) | None => Ok(None),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn localized_title(
    value: Option<&Value>,
) -> Result<Option<Localized<Language, Title>>, ProductListingEventReadError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = object(Some(value))?;
    Ok(Some(Localized::new(
        language(
            optional_string(value, "language")?
                .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        )?,
        Title::from(
            optional_string(value, "text")?
                .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        ),
    )))
}

fn localized_description(
    value: Option<&Value>,
) -> Result<Option<Localized<Language, Description>>, ProductListingEventReadError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = object(Some(value))?;
    Ok(Some(Localized::new(
        language(
            optional_string(value, "language")?
                .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        )?,
        Description::from(
            optional_string(value, "text")?
                .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        ),
    )))
}

fn pricing(value: Option<&Value>) -> Result<ProductListingPricing, ProductListingEventReadError> {
    let value = object(value)?;
    Ok(ProductListingPricing {
        price: price(value.get("price"))?,
        price_estimate_min: price(value.get("priceEstimateMin"))?,
        price_estimate_max: price(value.get("priceEstimateMax"))?,
    })
}

fn sale_observation(
    value: Option<&Value>,
) -> Result<ListingSaleObservation, ProductListingEventReadError> {
    let value = object(value)?;
    let observed_at = optional_string(value, "observedAt")?
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    let fx_rate_id = optional_string(value, "fxRateId")?
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?
        .parse::<uuid::Uuid>()
        .map(FxRateId::from)
        .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    Ok(ListingSaleObservation::new(
        OffsetDateTime::parse(observed_at, &Rfc3339)
            .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        fx_rate_id,
    ))
}

fn availability(
    value: Option<&Value>,
) -> Result<Option<ListingAvailability>, ProductListingEventReadError> {
    let Some(value) = value else {
        return Err(ProductListingEventReadError::ProductListingEventReadModelInvalid);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .and_then(ListingAvailability::from_code)
        .map(Some)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn price(value: Option<&Value>) -> Result<Option<Price>, ProductListingEventReadError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = object(Some(value))?;
    let amount = value
        .get("amount")
        .and_then(Value::as_u64)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    let currency_code = optional_string(value, "currency")?
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    Ok(Some(Price::new(
        MonetaryAmount::from(amount),
        currency(currency_code)?,
    )))
}

fn listing_source_id(payload: &Value) -> Result<ListingSourceId, ProductListingEventReadError> {
    string(payload, "listingSourceId")?
        .parse::<uuid::Uuid>()
        .map(ListingSourceId::from)
        .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn source_listing_id(payload: &Value) -> Result<SourceListingId, ProductListingEventReadError> {
    SourceListingId::try_from(string(payload, "sourceListingId")?)
        .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn images(
    value: Option<&Value>,
) -> Result<IndexSet<ProductListingImage>, ProductListingEventReadError> {
    value
        .and_then(Value::as_array)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?
        .iter()
        .map(|image| {
            let image = object(Some(image))?;
            Ok(ProductListingImage::new(url(optional_string(
                image, "url",
            )?
            .ok_or(
                ProductListingEventReadError::ProductListingEventReadModelInvalid,
            )?)?))
        })
        .collect()
}

fn auction(value: Option<&Value>) -> Result<ProductListingAuction, ProductListingEventReadError> {
    let value = object(value)?;
    Ok(ProductListingAuction {
        start: optional_string(value, "start")?
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        end: optional_string(value, "end")?
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
    })
}

fn url(value: &str) -> Result<Url, ProductListingEventReadError> {
    Url::parse(value).map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn language(value: &str) -> Result<Language, ProductListingEventReadError> {
    Language::from_code(value)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}

fn currency(value: &str) -> Result<Currency, ProductListingEventReadError> {
    Currency::from_code(value)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}
