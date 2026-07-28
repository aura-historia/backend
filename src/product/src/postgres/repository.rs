#![allow(dead_code)]

use crate::core::description::Description;
use crate::core::fx_rate_id::FxRateId;
use crate::core::product_aggregate::{
    Product, ProductAddress, ProductAuction, ProductPricing, ProductStateSnapshot,
};
use crate::core::product_image::ProductImage;
use crate::core::prohibited_content::ProhibitedContent;
use crate::core::title::Title;
use crate::service::ports::product_repository::{
    LoadedProduct, ProductRepository, ProductRepositoryError,
};
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::OffsetDateTime;
use url::Url;

pub(crate) struct SqlxProductRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductRow {
    product_id: uuid::Uuid,
    product_slug_id: String,
    event_id: uuid::Uuid,
    shop_id: uuid::Uuid,
    seller_id: uuid::Uuid,
    shops_product_id: String,
    structured_address_addressline: Option<String>,
    structured_address_addressline_extra: Option<String>,
    structured_address_locality: Option<String>,
    structured_address_region: Option<String>,
    structured_address_postal_code: Option<String>,
    structured_address_country: Option<String>,
    geo_address_lat: Option<f64>,
    geo_address_lon: Option<f64>,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    price_native_amount: Option<i64>,
    price_native_currency: Option<String>,
    price_estimate_min_native_amount: Option<i64>,
    price_estimate_min_native_currency: Option<String>,
    price_estimate_max_native_amount: Option<i64>,
    price_estimate_max_native_currency: Option<String>,
    fx_rate_id: Option<uuid::Uuid>,
    state: String,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProductImageJson {
    url: String,
    prohibited_content: String,
}

impl<'tx> SqlxProductRepository<'tx> {
    pub(crate) fn new(connection: &'tx mut PgConnection) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl ProductRepository for SqlxProductRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: ProductId,
    ) -> Result<Option<LoadedProduct>, ProductRepositoryError> {
        let row = sqlx::query_as::<_, ProductRow>(
            r#"
            SELECT
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_native_amount, price_native_currency, price_estimate_min_native_amount,
                price_estimate_min_native_currency, price_estimate_max_native_amount,
                price_estimate_max_native_currency, fx_rate_id, state, lifecycle, url, product_images,
                auction_start, auction_end, created, updated
            FROM products
            WHERE product_id = $1
            "#,
        )
        .bind(uuid::Uuid::from(id))
        .fetch_optional(&mut *self.connection)
        .await?;

        row.map(TryInto::try_into).transpose()
    }

    async fn find_by_key(
        &mut self,
        key: &ProductKey,
    ) -> Result<Option<LoadedProduct>, ProductRepositoryError> {
        let row = sqlx::query_as::<_, ProductRow>(
            r#"
            SELECT
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_native_amount, price_native_currency, price_estimate_min_native_amount,
                price_estimate_min_native_currency, price_estimate_max_native_amount,
                price_estimate_max_native_currency, fx_rate_id, state, lifecycle, url, product_images,
                auction_start, auction_end, created, updated
            FROM products
            WHERE shop_id = $1 AND shops_product_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(key.shop_id))
        .bind(key.shops_product_id.as_ref())
        .fetch_optional(&mut *self.connection)
        .await?;

        row.map(TryInto::try_into).transpose()
    }

    async fn insert(
        &mut self,
        product: &Product,
        current_event_id: EventId,
    ) -> Result<(), ProductRepositoryError> {
        let address = product.address();
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        sqlx::query(
            r#"
            INSERT INTO products (
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_native_amount, price_native_currency, price_estimate_min_native_amount,
                price_estimate_min_native_currency, price_estimate_max_native_amount,
                price_estimate_max_native_currency, fx_rate_id, state, lifecycle, url, product_images,
                auction_start, auction_end
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31
            )
            "#,
        )
        .bind(uuid::Uuid::from(product.id()))
        .bind(product.slug_id().as_ref().to_owned())
        .bind(uuid::Uuid::from(current_event_id))
        .bind(uuid::Uuid::from(product.shop_id()))
        .bind(uuid::Uuid::from(product.seller_id()))
        .bind(product.shops_product_id().as_ref().to_owned())
        .bind(address.structured.as_ref().and_then(|value| value.addressline.clone()))
        .bind(address.structured.as_ref().and_then(|value| value.addressline_extra.clone()))
        .bind(address.structured.as_ref().and_then(|value| value.locality.clone()))
        .bind(address.structured.as_ref().and_then(|value| value.region.clone()))
        .bind(address.structured.as_ref().and_then(|value| value.postal_code.clone()))
        .bind(address.structured.as_ref().and_then(|value| value.country.map(|country| country.alpha3().to_string())))
        .bind(address.geo.map(|value| value.lat))
        .bind(address.geo.map(|value| value.lon))
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(pricing.native_price.map(|value| amount_to_i64(value.monetary_amount)))
        .bind(pricing.native_price.map(|value| value.currency.as_str().to_owned()))
        .bind(pricing.native_price_estimate_min.map(|value| amount_to_i64(value.monetary_amount)))
        .bind(pricing.native_price_estimate_min.map(|value| value.currency.as_str().to_owned()))
        .bind(pricing.native_price_estimate_max.map(|value| amount_to_i64(value.monetary_amount)))
        .bind(pricing.native_price_estimate_max.map(|value| value.currency.as_str().to_owned()))
        .bind(pricing.fx_rate_id.map(uuid::Uuid::from))
        .bind(product_state_as_str(product.state()))
        .bind(product_lifecycle_as_str(product.lifecycle()))
        .bind(product.url().to_string())
        .bind(images_to_json(product.images()))
        .bind(auction.start)
        .bind(auction.end)
        .execute(&mut *self.connection)
        .await
        .map_err(map_insert_error)?;

        Ok(())
    }

    async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<(), ProductRepositoryError> {
        let address = product.address();
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        let result = sqlx::query(
            r#"
            UPDATE products
            SET
                product_slug_id = $1,
                event_id = $2,
                shop_id = $3,
                seller_id = $4,
                shops_product_id = $5,
                structured_address_addressline = $6,
                structured_address_addressline_extra = $7,
                structured_address_locality = $8,
                structured_address_region = $9,
                structured_address_postal_code = $10,
                structured_address_country = $11,
                geo_address_lat = $12,
                geo_address_lon = $13,
                title_text = $14,
                title_language = $15,
                description_text = $16,
                description_language = $17,
                price_native_amount = $18,
                price_native_currency = $19,
                price_estimate_min_native_amount = $20,
                price_estimate_min_native_currency = $21,
                price_estimate_max_native_amount = $22,
                price_estimate_max_native_currency = $23,
                fx_rate_id = $24,
                state = $25,
                lifecycle = $26,
                url = $27,
                product_images = $28,
                auction_start = $29,
                auction_end = $30,
                updated = now()
            WHERE product_id = $31 AND event_id = $32
            "#,
        )
        .bind(product.slug_id().as_ref().to_owned())
        .bind(uuid::Uuid::from(new_event_id))
        .bind(uuid::Uuid::from(product.shop_id()))
        .bind(uuid::Uuid::from(product.seller_id()))
        .bind(product.shops_product_id().as_ref().to_owned())
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.addressline.clone()),
        )
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.addressline_extra.clone()),
        )
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.locality.clone()),
        )
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.region.clone()),
        )
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.postal_code.clone()),
        )
        .bind(
            address
                .structured
                .as_ref()
                .and_then(|value| value.country.map(|country| country.alpha3().to_string())),
        )
        .bind(address.geo.map(|value| value.lat))
        .bind(address.geo.map(|value| value.lon))
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(
            pricing
                .native_price
                .map(|value| amount_to_i64(value.monetary_amount)),
        )
        .bind(
            pricing
                .native_price
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(
            pricing
                .native_price_estimate_min
                .map(|value| amount_to_i64(value.monetary_amount)),
        )
        .bind(
            pricing
                .native_price_estimate_min
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(
            pricing
                .native_price_estimate_max
                .map(|value| amount_to_i64(value.monetary_amount)),
        )
        .bind(
            pricing
                .native_price_estimate_max
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(pricing.fx_rate_id.map(uuid::Uuid::from))
        .bind(product_state_as_str(product.state()))
        .bind(product_lifecycle_as_str(product.lifecycle()))
        .bind(product.url().to_string())
        .bind(images_to_json(product.images()))
        .bind(auction.start)
        .bind(auction.end)
        .bind(uuid::Uuid::from(product.id()))
        .bind(uuid::Uuid::from(expected_event_id))
        .execute(&mut *self.connection)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ProductRepositoryError::ConcurrencyConflict);
        }

        Ok(())
    }
}

impl TryFrom<ProductRow> for LoadedProduct {
    type Error = ProductRepositoryError;

    fn try_from(row: ProductRow) -> Result<Self, Self::Error> {
        let _created = row.created;
        let _updated = row.updated;
        let address = ProductAddress {
            structured: structured_address_from_row(&row),
            geo: geo_address_from_row(&row),
        };
        let title = localized_title_from_row(&row)?;
        let description = localized_description_from_row(&row)?;
        let product = Product::rehydrate(ProductStateSnapshot {
            id: ProductId::from(row.product_id),
            slug_id: ProductSlugId::raw(&row.product_slug_id)
                .map_err(|_| ProductRepositoryError::InvalidPersistedState)?,
            shop_id: ShopId::from(row.shop_id),
            seller_id: ShopId::from(row.seller_id),
            shops_product_id: ShopsProductId::from(row.shops_product_id),
            address,
            title,
            description,
            pricing: ProductPricing {
                native_price: price_from_parts(row.price_native_amount, row.price_native_currency)?,
                native_price_estimate_min: price_from_parts(
                    row.price_estimate_min_native_amount,
                    row.price_estimate_min_native_currency,
                )?,
                native_price_estimate_max: price_from_parts(
                    row.price_estimate_max_native_amount,
                    row.price_estimate_max_native_currency,
                )?,
                fx_rate_id: row.fx_rate_id.map(FxRateId::from),
            },
            state: parse_product_state(&row.state)?,
            lifecycle: parse_product_lifecycle(&row.lifecycle)?,
            url: Url::parse(&row.url).map_err(|_| ProductRepositoryError::InvalidPersistedState)?,
            images: images_from_json(row.product_images)?,
            auction: ProductAuction {
                start: row.auction_start,
                end: row.auction_end,
            },
        })
        .map_err(|_| ProductRepositoryError::InvalidPersistedState)?;

        Ok(LoadedProduct {
            product,
            current_event_id: EventId::from(row.event_id),
        })
    }
}

fn amount_to_i64(amount: MonetaryAmount) -> i64 {
    i64::try_from(u64::from(amount)).unwrap_or(i64::MAX)
}

fn price_from_parts(
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<Price>, ProductRepositoryError> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => {
            let amount =
                u64::try_from(amount).map_err(|_| ProductRepositoryError::InvalidPersistedState)?;
            Ok(Some(Price::new(
                MonetaryAmount::from(amount),
                parse_currency(&currency)?,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn structured_address_from_row(row: &ProductRow) -> Option<StructuredAddress> {
    row.structured_address_addressline
        .as_ref()
        .map(|addressline| {
            let country = row
                .structured_address_country
                .as_deref()
                .and_then(|value| isocountry::CountryCode::for_alpha3(value).ok());
            StructuredAddress {
                addressline: Some(addressline.clone()),
                addressline_extra: row.structured_address_addressline_extra.clone(),
                locality: row.structured_address_locality.clone(),
                region: row.structured_address_region.clone(),
                postal_code: row.structured_address_postal_code.clone(),
                country,
                continent: country.map(geo::core::continent::Continent::from),
            }
        })
}

fn geo_address_from_row(row: &ProductRow) -> Option<GeoAddress> {
    match (row.geo_address_lat, row.geo_address_lon) {
        (Some(lat), Some(lon)) => Some(GeoAddress { lat, lon }),
        _ => None,
    }
}

fn localized_title_from_row(
    row: &ProductRow,
) -> Result<Option<Localized<Language, Title>>, ProductRepositoryError> {
    match (&row.title_text, &row.title_language) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            parse_language(language)?,
            Title::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn localized_description_from_row(
    row: &ProductRow,
) -> Result<Option<Localized<Language, Description>>, ProductRepositoryError> {
    match (&row.description_text, &row.description_language) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            parse_language(language)?,
            Description::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn images_to_json(images: &IndexSet<ProductImage>) -> serde_json::Value {
    let images = images
        .iter()
        .map(|image| ProductImageJson {
            url: image.url.to_string(),
            prohibited_content: image.prohibited_content.as_str().to_owned(),
        })
        .collect::<Vec<_>>();
    serde_json::to_value(images).unwrap_or_else(|_| serde_json::json!([]))
}

fn images_from_json(
    value: serde_json::Value,
) -> Result<IndexSet<ProductImage>, ProductRepositoryError> {
    let images: Vec<ProductImageJson> =
        serde_json::from_value(value).map_err(|_| ProductRepositoryError::InvalidPersistedState)?;
    images
        .into_iter()
        .map(|image| {
            Ok(ProductImage {
                url: Url::parse(&image.url)
                    .map_err(|_| ProductRepositoryError::InvalidPersistedState)?,
                prohibited_content: parse_prohibited_content(&image.prohibited_content)?,
            })
        })
        .collect()
}

fn parse_language(value: &str) -> Result<Language, ProductRepositoryError> {
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
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn parse_currency(value: &str) -> Result<Currency, ProductRepositoryError> {
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
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn parse_product_state(value: &str) -> Result<ProductState, ProductRepositoryError> {
    match value {
        "LISTED" => Ok(ProductState::Listed),
        "AVAILABLE" => Ok(ProductState::Available),
        "RESERVED" => Ok(ProductState::Reserved),
        "SOLD" => Ok(ProductState::Sold),
        "REMOVED" => Ok(ProductState::Removed),
        "UNKNOWN" => Ok(ProductState::Unknown),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn product_state_as_str(value: ProductState) -> &'static str {
    match value {
        ProductState::Listed => "LISTED",
        ProductState::Available => "AVAILABLE",
        ProductState::Reserved => "RESERVED",
        ProductState::Sold => "SOLD",
        ProductState::Removed => "REMOVED",
        ProductState::Unknown => "UNKNOWN",
    }
}

fn parse_product_lifecycle(value: &str) -> Result<ProductLifecycle, ProductRepositoryError> {
    match value {
        "ACTIVE" => Ok(ProductLifecycle::Active),
        "DELETED" => Ok(ProductLifecycle::Deleted),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn product_lifecycle_as_str(value: ProductLifecycle) -> &'static str {
    match value {
        ProductLifecycle::Active => "ACTIVE",
        ProductLifecycle::Deleted => "DELETED",
    }
}

fn parse_prohibited_content(value: &str) -> Result<ProhibitedContent, ProductRepositoryError> {
    match value {
        "UNKNOWN" => Ok(ProhibitedContent::Unknown),
        "NONE" => Ok(ProhibitedContent::None),
        "NAZI_GERMANY" => Ok(ProhibitedContent::NaziGermany),
        _ => Err(ProductRepositoryError::InvalidPersistedState),
    }
}

fn map_insert_error(error: sqlx::Error) -> ProductRepositoryError {
    match &error {
        sqlx::Error::Database(db_error)
            if db_error.constraint() == Some("products_shop_product_unique") =>
        {
            ProductRepositoryError::ProductKeyConflict
        }
        sqlx::Error::Database(db_error)
            if db_error.constraint() == Some("products_slug_unique") =>
        {
            ProductRepositoryError::SlugConflict
        }
        _ => ProductRepositoryError::from(error),
    }
}
