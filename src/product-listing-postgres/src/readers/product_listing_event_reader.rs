use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_state::ProductState;

use indexmap::IndexSet;
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingPricing, ProductSaleValuation,
};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::title::Title;
use product_listing_service::ports::{
    ProductListingEventReadError, ProductListingEventReader, ProductListingEventReaderFactory,
};
use product_listing_service::use_cases::{
    ProductListingAddressChangedEventPayload, ProductListingAuctionChangedEventPayload,
    ProductListingCreatedEventPayload, ProductListingDeletedEventPayload, ProductListingEvent,
    ProductListingEventLookup, ProductListingEventPayload, ProductListingEventType,
    ProductListingImagesChangedEventPayload, ProductListingPriceChangedEventPayload,
    ProductListingStateChangedEventPayload, ProductListingUrlChangedEventPayload,
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
    product_id: uuid::Uuid,
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
        let product_id = match lookup {
            ProductListingEventLookup::ById(product_id) => sqlx::query_scalar::<_, uuid::Uuid>(
                "SELECT product_id FROM products WHERE product_id = $1",
            )
            .bind(uuid::Uuid::from(*product_id))
            .fetch_optional(&mut *self.connection)
            .await,
            ProductListingEventLookup::BySlug { shop_slug_id, product_slug_id } => sqlx::query_scalar::<_, uuid::Uuid>(
                "SELECT p.product_id FROM products p JOIN shops s ON s.shop_id = p.shop_id WHERE s.shop_slug_id = $1 AND p.product_slug_id = $2",
            )
            .bind(shop_slug_id.as_ref())
            .bind(product_slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await,
        }
        .map_err(|_| ProductListingEventReadError::ProductListingEventQueryFailed)?;

        let Some(product_id) = product_id else {
            return Ok(None);
        };

        let rows = sqlx::query_as::<_, ProductListingEventRow>(
            r#"
            SELECT event_id, product_id, event_type, payload, event_time
            FROM product_events
            WHERE product_id = $1
              AND event_group = 'DOMAIN'
            ORDER BY event_time ASC, event_id ASC
            "#,
        )
        .bind(product_id)
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
        let (event_type, payload) = parse_payload(&row.event_type, &row.payload)?;
        Ok(ProductListingEvent {
            product_id: ProductListingId::from(row.product_id),
            event_id: EventId::from(row.event_id),
            event_type,
            payload,
            timestamp: row.event_time,
        })
    }
}

pub(crate) fn parse_payload(
    event_type: &str,
    payload: &Value,
) -> Result<(ProductListingEventType, ProductListingEventPayload), ProductListingEventReadError> {
    match event_type {
        "PRODUCT_CREATED" => Ok((
            ProductListingEventType::Created,
            ProductListingEventPayload::Created(ProductListingCreatedEventPayload {
                title: localized_title(payload.get("title"))?,
                description: localized_description(payload.get("description"))?,
                address: address(payload.get("address"))?,
                pricing: pricing(payload.get("pricing"))?,
                sale_valuation: sale_valuation(payload.get("saleValuation"))?,
                state: state(string(payload, "state")?)?,
                url: url(string(payload, "url")?)?,
                images: images(payload.get("images"))?,
                auction: auction(payload.get("auction"))?,
            }),
        )),
        "PRODUCT_STATE_CHANGED" => Ok((
            ProductListingEventType::StateChanged,
            ProductListingEventPayload::StateChanged(ProductListingStateChangedEventPayload {
                old_state: state(string(payload, "oldState")?)?,
                new_state: state(string(payload, "newState")?)?,
                sale_valuation: sale_valuation(payload.get("saleValuation"))?,
            }),
        )),
        "PRODUCT_ADDRESS_CHANGED" => Ok((
            ProductListingEventType::AddressChanged,
            ProductListingEventPayload::AddressChanged(ProductListingAddressChangedEventPayload {
                address: address(payload.get("address"))?,
            }),
        )),
        "PRODUCT_PRICE_CHANGED" => Ok((
            ProductListingEventType::PriceChanged,
            ProductListingEventPayload::PriceChanged(ProductListingPriceChangedEventPayload {
                old_pricing: pricing(payload.get("oldPricing"))?,
                new_pricing: pricing(payload.get("newPricing"))?,
            }),
        )),
        "PRODUCT_URL_CHANGED" => Ok((
            ProductListingEventType::UrlChanged,
            ProductListingEventPayload::UrlChanged(ProductListingUrlChangedEventPayload {
                old_url: url(string(payload, "oldUrl")?)?,
                new_url: url(string(payload, "newUrl")?)?,
            }),
        )),
        "PRODUCT_IMAGES_CHANGED" => Ok((
            ProductListingEventType::ImagesChanged,
            ProductListingEventPayload::ImagesChanged(ProductListingImagesChangedEventPayload {
                images: images(payload.get("images"))?,
            }),
        )),
        "PRODUCT_AUCTION_CHANGED" => Ok((
            ProductListingEventType::AuctionChanged,
            ProductListingEventPayload::AuctionChanged(ProductListingAuctionChangedEventPayload {
                auction: auction(payload.get("auction"))?,
            }),
        )),
        "PRODUCT_DELETED" => Ok((
            ProductListingEventType::Deleted,
            ProductListingEventPayload::Deleted(ProductListingDeletedEventPayload {
                old_lifecycle: lifecycle(string(payload, "oldLifecycle")?)?,
                new_lifecycle: lifecycle(string(payload, "newLifecycle")?)?,
            }),
        )),
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

fn sale_valuation(
    value: Option<&Value>,
) -> Result<Option<ProductSaleValuation>, ProductListingEventReadError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = object(Some(value))?;
    let sold_at = optional_string(value, "soldAt")?
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    let fx_rate_id = optional_string(value, "fxRateId")?
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?
        .parse::<uuid::Uuid>()
        .map(FxRateId::from)
        .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
    Ok(Some(ProductSaleValuation {
        sold_at: OffsetDateTime::parse(sold_at, &Rfc3339)
            .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
        fx_rate_id,
    }))
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

fn address(value: Option<&Value>) -> Result<ProductListingAddress, ProductListingEventReadError> {
    let value = object(value)?;
    let structured = match value.get("structured") {
        Some(Value::Null) | None => None,
        Some(structured) => {
            let structured = object(Some(structured))?;
            let country = optional_string(structured, "country")?
                .map(isocountry::CountryCode::for_alpha3)
                .transpose()
                .map_err(|_| ProductListingEventReadError::ProductListingEventReadModelInvalid)?;
            Some(geo::core::address::StructuredAddress {
                addressline: optional_string(structured, "addressline")?.map(str::to_owned),
                addressline_extra: optional_string(structured, "addresslineExtra")?
                    .map(str::to_owned),
                locality: optional_string(structured, "locality")?.map(str::to_owned),
                region: optional_string(structured, "region")?.map(str::to_owned),
                postal_code: optional_string(structured, "postalCode")?.map(str::to_owned),
                country,
                continent: country.map(geo::core::continent::Continent::from),
            })
        }
    };
    let geo = match value.get("geo") {
        Some(Value::Null) | None => None,
        Some(geo) => {
            let geo = object(Some(geo))?;
            Some(geo::core::address::GeoAddress {
                lat: geo
                    .get("lat")
                    .and_then(Value::as_f64)
                    .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
                lon: geo
                    .get("lon")
                    .and_then(Value::as_f64)
                    .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
            })
        }
    };
    Ok(ProductListingAddress { structured, geo })
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
            Ok(ProductListingImage {
                url: url(optional_string(image, "url")?
                    .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?)?,
                prohibited_content: prohibited_content(
                    optional_string(image, "prohibitedContent")?
                        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)?,
                )?,
            })
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
    match value.to_ascii_lowercase().as_str() {
        "de" => Ok(Language::De),
        "en" => Ok(Language::En),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        "it" => Ok(Language::It),
        "zh" => Ok(Language::Zh),
        "pt" => Ok(Language::Pt),
        "pl" => Ok(Language::Pl),
        "tr" => Ok(Language::Tr),
        "nl" => Ok(Language::Nl),
        "cs" => Ok(Language::Cs),
        "ja" => Ok(Language::Ja),
        "ru" => Ok(Language::Ru),
        "ar" => Ok(Language::Ar),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn currency(value: &str) -> Result<Currency, ProductListingEventReadError> {
    match value.to_ascii_uppercase().as_str() {
        "EUR" => Ok(Currency::Eur),
        "GBP" => Ok(Currency::Gbp),
        "USD" => Ok(Currency::Usd),
        "AUD" => Ok(Currency::Aud),
        "CAD" => Ok(Currency::Cad),
        "NZD" => Ok(Currency::Nzd),
        "CNY" => Ok(Currency::Cny),
        "BRL" => Ok(Currency::Brl),
        "PLN" => Ok(Currency::Pln),
        "TRY" => Ok(Currency::Try),
        "JPY" => Ok(Currency::Jpy),
        "CZK" => Ok(Currency::Czk),
        "RUB" => Ok(Currency::Rub),
        "AED" => Ok(Currency::Aed),
        "SAR" => Ok(Currency::Sar),
        "HKD" => Ok(Currency::Hkd),
        "SGD" => Ok(Currency::Sgd),
        "CHF" => Ok(Currency::Chf),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn state(value: &str) -> Result<ProductState, ProductListingEventReadError> {
    match value {
        "Listed" => Ok(ProductState::Listed),
        "Available" => Ok(ProductState::Available),
        "Reserved" => Ok(ProductState::Reserved),
        "Sold" => Ok(ProductState::Sold),
        "Removed" => Ok(ProductState::Removed),
        "Unknown" => Ok(ProductState::Unknown),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn lifecycle(value: &str) -> Result<ProductLifecycle, ProductListingEventReadError> {
    match value {
        "Active" => Ok(ProductLifecycle::Active),
        "Deleted" => Ok(ProductLifecycle::Deleted),
        _ => Err(ProductListingEventReadError::ProductListingEventReadModelInvalid),
    }
}

fn prohibited_content(value: &str) -> Result<ProhibitedContent, ProductListingEventReadError> {
    ProhibitedContent::from_code(value)
        .ok_or(ProductListingEventReadError::ProductListingEventReadModelInvalid)
}
