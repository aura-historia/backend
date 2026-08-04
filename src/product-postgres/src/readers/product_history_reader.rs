use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;

use indexmap::IndexSet;
use product_core::description::Description;
use product_core::fx_rate_id::FxRateId;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_service::ports::{
    ProductHistoryReadError, ProductHistoryReader, ProductHistoryReaderFactory,
};
use product_service::use_cases::{
    ProductAddressChangedHistoryPayload, ProductAuctionChangedHistoryPayload,
    ProductCreatedHistoryPayload, ProductDeletedHistoryPayload, ProductHistoryEvent,
    ProductHistoryEventType, ProductHistoryPayload, ProductImagesChangedHistoryPayload,
    ProductPriceChangedHistoryPayload, ProductStateChangedHistoryPayload,
    ProductUrlChangedHistoryPayload,
};
use serde_json::Value;
use sqlx::PgConnection;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductHistoryReaderFactory;

struct SqlxProductHistoryReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductHistoryRow {
    product_id: uuid::Uuid,
    event_id: uuid::Uuid,
    event_type: String,
    event_type_schema_version: i32,
    payload: Value,
    event_time: OffsetDateTime,
}

impl SqlxProductHistoryReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductHistoryReaderFactory<common::postgres::SqlxTransaction>
    for SqlxProductHistoryReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl ProductHistoryReader + 'tx {
        SqlxProductHistoryReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductHistoryReader for SqlxProductHistoryReader<'_> {
    async fn find_history(
        &mut self,
        product_key: &ProductKey,
    ) -> Result<Option<Vec<ProductHistoryEvent>>, ProductHistoryReadError> {
        let product_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT product_id FROM products WHERE shop_id = $1 AND shops_product_id = $2",
        )
        .bind(uuid::Uuid::from(product_key.shop_id))
        .bind(product_key.shops_product_id.as_ref())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|_| ProductHistoryReadError::ProductHistoryQueryFailed)?;

        let Some(product_id) = product_id else {
            return Ok(None);
        };

        let rows = sqlx::query_as::<_, ProductHistoryRow>(
            r#"
            SELECT event_id, product_id, event_type, event_type_schema_version, payload, event_time
            FROM product_events
            WHERE product_id = $1
            ORDER BY event_time ASC, event_id ASC
            "#,
        )
        .bind(product_id)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|_| ProductHistoryReadError::ProductHistoryQueryFailed)?;

        rows.into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

impl TryFrom<ProductHistoryRow> for ProductHistoryEvent {
    type Error = ProductHistoryReadError;

    fn try_from(row: ProductHistoryRow) -> Result<Self, Self::Error> {
        if row.event_type_schema_version != 2 || payload_version(&row.payload) != Some(2) {
            return Err(ProductHistoryReadError::UnsupportedProductHistoryEventSchema);
        }

        let (event_type, payload) = parse_payload(&row.event_type, &row.payload)?;
        Ok(ProductHistoryEvent {
            product_id: ProductId::from(row.product_id),
            event_id: EventId::from(row.event_id),
            event_type,
            payload,
            timestamp: row.event_time,
        })
    }
}

fn payload_version(payload: &Value) -> Option<i64> {
    payload.get("version").and_then(Value::as_i64)
}

fn parse_payload(
    event_type: &str,
    payload: &Value,
) -> Result<(ProductHistoryEventType, ProductHistoryPayload), ProductHistoryReadError> {
    match event_type {
        "PRODUCT_CREATED" => Ok((
            ProductHistoryEventType::Created,
            ProductHistoryPayload::Created(ProductCreatedHistoryPayload {
                title: localized_title(payload.get("title"))?,
                description: localized_description(payload.get("description"))?,
                address: address(payload.get("address"))?,
                pricing: pricing(payload.get("pricing"))?,
                state: state(string(payload, "state")?)?,
                url: url(string(payload, "url")?)?,
                images: images(payload.get("images"))?,
                auction: auction(payload.get("auction"))?,
            }),
        )),
        "PRODUCT_STATE_CHANGED" => Ok((
            ProductHistoryEventType::StateChanged,
            ProductHistoryPayload::StateChanged(ProductStateChangedHistoryPayload {
                old_state: state(string(payload, "oldState")?)?,
                new_state: state(string(payload, "newState")?)?,
            }),
        )),
        "PRODUCT_ADDRESS_CHANGED" => Ok((
            ProductHistoryEventType::AddressChanged,
            ProductHistoryPayload::AddressChanged(ProductAddressChangedHistoryPayload {
                address: address(payload.get("address"))?,
            }),
        )),
        "PRODUCT_PRICE_CHANGED" => Ok((
            ProductHistoryEventType::PriceChanged,
            ProductHistoryPayload::PriceChanged(ProductPriceChangedHistoryPayload {
                old_pricing: pricing(payload.get("oldPricing"))?,
                new_pricing: pricing(payload.get("newPricing"))?,
            }),
        )),
        "PRODUCT_URL_CHANGED" => Ok((
            ProductHistoryEventType::UrlChanged,
            ProductHistoryPayload::UrlChanged(ProductUrlChangedHistoryPayload {
                old_url: url(string(payload, "oldUrl")?)?,
                new_url: url(string(payload, "newUrl")?)?,
            }),
        )),
        "PRODUCT_IMAGES_CHANGED" => Ok((
            ProductHistoryEventType::ImagesChanged,
            ProductHistoryPayload::ImagesChanged(ProductImagesChangedHistoryPayload {
                images: images(payload.get("images"))?,
            }),
        )),
        "PRODUCT_AUCTION_CHANGED" => Ok((
            ProductHistoryEventType::AuctionChanged,
            ProductHistoryPayload::AuctionChanged(ProductAuctionChangedHistoryPayload {
                auction: auction(payload.get("auction"))?,
            }),
        )),
        "PRODUCT_DELETED" => Ok((
            ProductHistoryEventType::Deleted,
            ProductHistoryPayload::Deleted(ProductDeletedHistoryPayload {
                old_lifecycle: lifecycle(string(payload, "oldLifecycle")?)?,
                new_lifecycle: lifecycle(string(payload, "newLifecycle")?)?,
            }),
        )),
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn object(
    value: Option<&Value>,
) -> Result<&serde_json::Map<String, Value>, ProductHistoryReadError> {
    value
        .and_then(Value::as_object)
        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, ProductHistoryReadError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)
}

fn optional_string<'a>(
    value: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, ProductHistoryReadError> {
    match value.get(name) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(Value::Null) | None => Ok(None),
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn localized_title(
    value: Option<&Value>,
) -> Result<Option<Localized<Language, Title>>, ProductHistoryReadError> {
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
                .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
        )?,
        Title::from(
            optional_string(value, "text")?
                .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
        ),
    )))
}

fn localized_description(
    value: Option<&Value>,
) -> Result<Option<Localized<Language, Description>>, ProductHistoryReadError> {
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
                .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
        )?,
        Description::from(
            optional_string(value, "text")?
                .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
        ),
    )))
}

fn pricing(value: Option<&Value>) -> Result<ProductPricing, ProductHistoryReadError> {
    let value = object(value)?;
    Ok(ProductPricing {
        price: price(value.get("price"))?,
        price_estimate_min: price(value.get("priceEstimateMin"))?,
        price_estimate_max: price(value.get("priceEstimateMax"))?,
        fx_rate_id: optional_string(value, "fxRateId")?
            .map(|value| value.parse::<uuid::Uuid>().map(FxRateId::from))
            .transpose()
            .map_err(|_| ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
    })
}

fn price(value: Option<&Value>) -> Result<Option<Price>, ProductHistoryReadError> {
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
        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?;
    let currency_code = optional_string(value, "currency")?
        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?;
    Ok(Some(Price::new(
        MonetaryAmount::from(amount),
        currency(currency_code)?,
    )))
}

fn address(value: Option<&Value>) -> Result<ProductAddress, ProductHistoryReadError> {
    let value = object(value)?;
    let structured = match value.get("structured") {
        Some(Value::Null) | None => None,
        Some(structured) => {
            let structured = object(Some(structured))?;
            let country = optional_string(structured, "country")?
                .map(isocountry::CountryCode::for_alpha3)
                .transpose()
                .map_err(|_| ProductHistoryReadError::ProductHistoryReadModelInvalid)?;
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
                    .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
                lon: geo
                    .get("lon")
                    .and_then(Value::as_f64)
                    .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
            })
        }
    };
    Ok(ProductAddress { structured, geo })
}

fn images(value: Option<&Value>) -> Result<IndexSet<ProductImage>, ProductHistoryReadError> {
    value
        .and_then(Value::as_array)
        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?
        .iter()
        .map(|image| {
            let image = object(Some(image))?;
            Ok(ProductImage {
                url: url(optional_string(image, "url")?
                    .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?)?,
                prohibited_content: prohibited_content(
                    optional_string(image, "prohibitedContent")?
                        .ok_or(ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
                )?,
            })
        })
        .collect()
}

fn auction(value: Option<&Value>) -> Result<ProductAuction, ProductHistoryReadError> {
    let value = object(value)?;
    Ok(ProductAuction {
        start: optional_string(value, "start")?
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .map_err(|_| ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
        end: optional_string(value, "end")?
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .map_err(|_| ProductHistoryReadError::ProductHistoryReadModelInvalid)?,
    })
}

fn url(value: &str) -> Result<Url, ProductHistoryReadError> {
    Url::parse(value).map_err(|_| ProductHistoryReadError::ProductHistoryReadModelInvalid)
}

fn language(value: &str) -> Result<Language, ProductHistoryReadError> {
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
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn currency(value: &str) -> Result<Currency, ProductHistoryReadError> {
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
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn state(value: &str) -> Result<ProductState, ProductHistoryReadError> {
    match value {
        "Listed" => Ok(ProductState::Listed),
        "Available" => Ok(ProductState::Available),
        "Reserved" => Ok(ProductState::Reserved),
        "Sold" => Ok(ProductState::Sold),
        "Removed" => Ok(ProductState::Removed),
        "Unknown" => Ok(ProductState::Unknown),
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn lifecycle(value: &str) -> Result<ProductLifecycle, ProductHistoryReadError> {
    match value {
        "Active" => Ok(ProductLifecycle::Active),
        "Deleted" => Ok(ProductLifecycle::Deleted),
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}

fn prohibited_content(value: &str) -> Result<ProhibitedContent, ProductHistoryReadError> {
    match value {
        "UNKNOWN" => Ok(ProhibitedContent::Unknown),
        "NONE" => Ok(ProhibitedContent::None),
        "NAZI_GERMANY" => Ok(ProhibitedContent::NaziGermany),
        _ => Err(ProductHistoryReadError::ProductHistoryReadModelInvalid),
    }
}
