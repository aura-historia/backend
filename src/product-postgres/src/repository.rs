#![allow(dead_code)]

use common::currency::domain::Currency;
use common::error::boxed::box_error;
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
use common::versioned::Versioned;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{
    Product, ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
    RehydratedProductState,
};
use product_core::product_image::ProductImage;
use product_core::prohibited_content::ProhibitedContent;
use product_core::title::Title;
use product_service::ports::product_repository::{
    ProductRepository, ProductRepositoryError, ProductRepositoryFactory,
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductRepositoryFactory;

struct SqlxProductRepository<'tx> {
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
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    sale_fx_rate_id: Option<uuid::Uuid>,
    sold_at: Option<OffsetDateTime>,
    state: String,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    embedding: Option<Vec<f32>>,
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

impl SqlxProductRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductRepositoryFactory<common::postgres::SqlxTransaction> for SqlxProductRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl ProductRepository + 'tx {
        SqlxProductRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductRepository for SqlxProductRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: ProductId,
    ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError> {
        let row = sqlx::query_as::<_, ProductRow>(
            r#"
            SELECT
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_fx_rate_id, sold_at, state, lifecycle, url,
                product_images, embedding, auction_start, auction_end, created, updated
            FROM products
            WHERE product_id = $1
            "#,
        )
        .bind(uuid::Uuid::from(id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductLookupByIdSqlxError)?;

        row.map(TryInto::try_into).transpose()
    }

    async fn find_by_key(
        &mut self,
        key: &ProductKey,
    ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError> {
        let row = sqlx::query_as::<_, ProductRow>(
            r#"
            SELECT
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_fx_rate_id, sold_at, state, lifecycle, url,
                product_images, embedding, auction_start, auction_end, created, updated
            FROM products
            WHERE shop_id = $1
              AND shops_product_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(key.shop_id))
        .bind(key.shops_product_id.as_ref())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductLookupByKeySqlxError)?;

        row.map(TryInto::try_into).transpose()
    }

    async fn insert(
        &mut self,
        product: &Product,
        current_event_id: EventId,
    ) -> Result<Versioned<Product, EventId>, ProductRepositoryError> {
        let address = product.address();
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        let price_amount = pricing
            .price
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductInsertFailed)?;
        let price_estimate_min_amount = pricing
            .price_estimate_min
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductInsertFailed)?;
        let price_estimate_max_amount = pricing
            .price_estimate_max
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductInsertFailed)?;
        let product_images = images_to_json(product.images())
            .map_err(|_| ProductRepositoryError::ProductInsertFailed)?;
        sqlx::query(
            r#"
            INSERT INTO products (
                product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id,
                structured_address_addressline, structured_address_addressline_extra,
                structured_address_locality, structured_address_region, structured_address_postal_code,
                structured_address_country, geo_address_lat, geo_address_lon, title_text,
                title_language, description_text, description_language,
                price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_fx_rate_id, sold_at, state, lifecycle, url,
                product_images, auction_start, auction_end
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32
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
        .bind(price_amount)
        .bind(pricing.price.map(|value| value.currency.as_str().to_owned()))
        .bind(price_estimate_min_amount)
        .bind(
            pricing
                .price_estimate_min
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_max_amount)
        .bind(
            pricing
                .price_estimate_max
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(product.sale_valuation().map(|value| uuid::Uuid::from(value.fx_rate_id)))
        .bind(product.sale_valuation().map(|value| value.sold_at))
        .bind(product_state_as_str(product.state()))
        .bind(product_lifecycle_as_str(product.lifecycle()))
        .bind(product.url().to_string())
        .bind(product_images)
        .bind(auction.start)
        .bind(auction.end)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductInsertSqlxError)?;

        Ok(Versioned::new(product.clone(), current_event_id))
    }

    async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<Versioned<Product, EventId>, ProductRepositoryError> {
        let address = product.address();
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        let price_amount = pricing
            .price
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductUpdateFailed)?;
        let price_estimate_min_amount = pricing
            .price_estimate_min
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductUpdateFailed)?;
        let price_estimate_max_amount = pricing
            .price_estimate_max
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductRepositoryError::ProductUpdateFailed)?;
        let product_images = images_to_json(product.images())
            .map_err(|_| ProductRepositoryError::ProductUpdateFailed)?;
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
                price_amount = $18,
                price_currency = $19,
                price_estimate_min_amount = $20,
                price_estimate_min_currency = $21,
                price_estimate_max_amount = $22,
                price_estimate_max_currency = $23,
                sale_fx_rate_id = $24,
                sold_at = $25,
                state = $26,
                lifecycle = $27,
                url = $28,
                product_images = $29,
                auction_start = $30,
                auction_end = $31,
                projection_version = projection_version + 1,
                updated = now()
            WHERE product_id = $32 AND event_id = $33
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
        .bind(price_amount)
        .bind(
            pricing
                .price
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_min_amount)
        .bind(
            pricing
                .price_estimate_min
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_max_amount)
        .bind(
            pricing
                .price_estimate_max
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(
            product
                .sale_valuation()
                .map(|value| uuid::Uuid::from(value.fx_rate_id)),
        )
        .bind(product.sale_valuation().map(|value| value.sold_at))
        .bind(product_state_as_str(product.state()))
        .bind(product_lifecycle_as_str(product.lifecycle()))
        .bind(product.url().to_string())
        .bind(product_images)
        .bind(auction.start)
        .bind(auction.end)
        .bind(uuid::Uuid::from(product.id()))
        .bind(uuid::Uuid::from(expected_event_id))
        .execute(&mut *self.connection)
        .await
        .map_err(ProductUpdateSqlxError)?;

        if result.rows_affected() == 0 {
            return Err(ProductRepositoryError::ProductCurrentEventIdConflict);
        }

        Ok(Versioned::new(product.clone(), new_event_id))
    }
}

impl TryFrom<ProductRow> for Versioned<Product, EventId> {
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
        let product = Product::rehydrate(RehydratedProductState {
            id: ProductId::from(row.product_id),
            slug_id: ProductSlugId::raw(&row.product_slug_id)
                .map_err(|_| ProductRepositoryError::InvalidProductSlugPersisted)?,
            shop_id: ShopId::from(row.shop_id),
            seller_id: ShopId::from(row.seller_id),
            shops_product_id: ShopsProductId::from(row.shops_product_id),
            address,
            title,
            description,
            pricing: ProductPricing {
                price: price_from_parts(row.price_amount, row.price_currency)?,
                price_estimate_min: price_from_parts(
                    row.price_estimate_min_amount,
                    row.price_estimate_min_currency,
                )?,
                price_estimate_max: price_from_parts(
                    row.price_estimate_max_amount,
                    row.price_estimate_max_currency,
                )?,
            },
            sale_valuation: sale_valuation_from_parts(row.sold_at, row.sale_fx_rate_id)?,
            state: parse_product_state(&row.state)?,
            lifecycle: parse_product_lifecycle(&row.lifecycle)?,
            url: Url::parse(&row.url)
                .map_err(|_| ProductRepositoryError::InvalidProductUrlPersisted)?,
            images: images_from_json(row.product_images)?,
            auction: ProductAuction {
                start: row.auction_start,
                end: row.auction_end,
            },
        })
        .map_err(|_| ProductRepositoryError::InvalidAggregateStatePersisted)?;

        Ok(Versioned {
            value: product,
            version: EventId::from(row.event_id),
        })
    }
}

fn sale_valuation_from_parts(
    sold_at: Option<OffsetDateTime>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<ProductSaleValuation>, ProductRepositoryError> {
    match (sold_at, fx_rate_id) {
        (Some(sold_at), Some(fx_rate_id)) => Ok(Some(ProductSaleValuation {
            sold_at,
            fx_rate_id: common::fx_rate_id::FxRateId::from(fx_rate_id),
        })),
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::InvalidAggregateStatePersisted),
    }
}

fn amount_to_i64(amount: MonetaryAmount) -> Result<i64, ()> {
    i64::try_from(u64::from(amount)).map_err(|_| ())
}

fn price_from_parts(
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<Price>, ProductRepositoryError> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => {
            let amount = u64::try_from(amount)
                .map_err(|_| ProductRepositoryError::NegativePriceAmountPersisted)?;
            Ok(Some(Price::new(
                MonetaryAmount::from(amount),
                parse_currency(&currency)?,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::IncompletePricePersisted),
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
            parse_title_language(language)?,
            Title::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::IncompleteTitlePersisted),
    }
}

fn localized_description_from_row(
    row: &ProductRow,
) -> Result<Option<Localized<Language, Description>>, ProductRepositoryError> {
    match (&row.description_text, &row.description_language) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            parse_description_language(language)?,
            Description::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductRepositoryError::IncompleteDescriptionPersisted),
    }
}

fn images_to_json(images: &IndexSet<ProductImage>) -> Result<serde_json::Value, ()> {
    let images = images
        .iter()
        .map(|image| ProductImageJson {
            url: image.url.to_string(),
            prohibited_content: image.prohibited_content.as_str().to_owned(),
        })
        .collect::<Vec<_>>();
    serde_json::to_value(images).map_err(|_| ())
}

fn images_from_json(
    value: serde_json::Value,
) -> Result<IndexSet<ProductImage>, ProductRepositoryError> {
    let images: Vec<ProductImageJson> = serde_json::from_value(value)
        .map_err(|_| ProductRepositoryError::InvalidProductImagesPersisted)?;
    images
        .into_iter()
        .map(|image| {
            Ok(ProductImage {
                url: Url::parse(&image.url)
                    .map_err(|_| ProductRepositoryError::InvalidProductImageUrlPersisted)?,
                prohibited_content: parse_prohibited_content(&image.prohibited_content)?,
            })
        })
        .collect()
}

fn parse_title_language(value: &str) -> Result<Language, ProductRepositoryError> {
    parse_language(value, ProductRepositoryError::InvalidTitleLanguagePersisted)
}

fn parse_description_language(value: &str) -> Result<Language, ProductRepositoryError> {
    parse_language(
        value,
        ProductRepositoryError::InvalidDescriptionLanguagePersisted,
    )
}

fn parse_language(
    value: &str,
    error: ProductRepositoryError,
) -> Result<Language, ProductRepositoryError> {
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
        _ => Err(error),
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
        _ => Err(ProductRepositoryError::InvalidPriceCurrencyPersisted),
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
        _ => Err(ProductRepositoryError::InvalidProductStatePersisted),
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
        _ => Err(ProductRepositoryError::InvalidProductLifecyclePersisted),
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
        _ => Err(ProductRepositoryError::InvalidProductImageProhibitedContentPersisted),
    }
}

struct ProductLookupByIdSqlxError(sqlx::Error);
#[derive(Debug, thiserror::Error)]
#[error("product lookup by shop product identity query failed")]
struct ProductLookupByKeySqlxError(#[source] sqlx::Error);
struct ProductInsertSqlxError(sqlx::Error);
struct ProductUpdateSqlxError(sqlx::Error);

impl From<ProductLookupByIdSqlxError> for ProductRepositoryError {
    fn from(value: ProductLookupByIdSqlxError) -> Self {
        let ProductLookupByIdSqlxError(_error) = value;
        Self::ProductLookupByIdFailed
    }
}

impl From<ProductLookupByKeySqlxError> for ProductRepositoryError {
    fn from(error: ProductLookupByKeySqlxError) -> Self {
        Self::ProductLookupByKeyFailed {
            source: box_error(error),
        }
    }
}

impl From<ProductInsertSqlxError> for ProductRepositoryError {
    fn from(value: ProductInsertSqlxError) -> Self {
        let ProductInsertSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error)
                if db_error.constraint() == Some("products_shop_product_unique") =>
            {
                Self::ShopProductAlreadyExists
            }
            sqlx::Error::Database(db_error)
                if db_error.constraint() == Some("products_slug_unique") =>
            {
                Self::ProductSlugAlreadyExists
            }
            _ => Self::ProductInsertFailed,
        }
    }
}

impl From<ProductUpdateSqlxError> for ProductRepositoryError {
    fn from(value: ProductUpdateSqlxError) -> Self {
        let ProductUpdateSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error)
                if db_error.constraint() == Some("products_shop_product_unique") =>
            {
                Self::ShopProductAlreadyExists
            }
            sqlx::Error::Database(db_error)
                if db_error.constraint() == Some("products_slug_unique") =>
            {
                Self::ProductSlugAlreadyExists
            }
            _ => Self::ProductUpdateFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::event_id::EventId;
    use serde_json::json;

    #[test]
    fn should_preserve_key_lookup_sqlx_source() {
        let error = ProductLookupByKeySqlxError(sqlx::Error::Protocol("test failure".to_owned()));

        let mapped: ProductRepositoryError = error.into();

        let source = match mapped {
            ProductRepositoryError::ProductLookupByKeyFailed { source } => source,
            error => panic!("unexpected error: {error:?}"),
        };
        assert_eq!(
            "product lookup by shop product identity query failed",
            source.to_string()
        );
        assert!(
            std::error::Error::source(source.as_ref())
                .is_some_and(|error| error.to_string().contains("test failure"))
        );
    }

    #[test]
    fn should_map_complete_and_empty_prices_from_parts() {
        let price = match price_from_parts(Some(123), Some("eur".to_owned())) {
            Ok(Some(price)) => price,
            Ok(None) => panic!("missing mapped price"),
            Err(error) => panic!("failed to map price: {error:?}"),
        };
        let empty = match price_from_parts(None, None) {
            Ok(value) => value,
            Err(error) => panic!("failed to map empty price: {error:?}"),
        };

        assert_eq!(MonetaryAmount::from(123_u64), price.monetary_amount);
        assert_eq!(Currency::Eur, price.currency);
        assert_eq!(None, empty);
    }

    #[test]
    fn should_reject_incomplete_negative_and_invalid_price_parts() {
        assert!(matches!(
            price_from_parts(Some(123), None),
            Err(ProductRepositoryError::IncompletePricePersisted)
        ));
        assert!(matches!(
            price_from_parts(Some(-1), Some("EUR".to_owned())),
            Err(ProductRepositoryError::NegativePriceAmountPersisted)
        ));
        assert!(matches!(
            price_from_parts(Some(123), Some("NOPE".to_owned())),
            Err(ProductRepositoryError::InvalidPriceCurrencyPersisted)
        ));
    }

    #[test]
    fn should_map_optional_address_language_and_image_branches() {
        let row = product_row();
        let structured = match structured_address_from_row(&row) {
            Some(value) => value,
            None => panic!("missing structured address"),
        };
        let geo = match geo_address_from_row(&row) {
            Some(value) => value,
            None => panic!("missing geo address"),
        };
        let title = match localized_title_from_row(&row) {
            Ok(Some(value)) => value,
            Ok(None) => panic!("missing title"),
            Err(error) => panic!("failed to map title: {error:?}"),
        };
        let description = match localized_description_from_row(&row) {
            Ok(Some(value)) => value,
            Ok(None) => panic!("missing description"),
            Err(error) => panic!("failed to map description: {error:?}"),
        };
        let images = match images_from_json(row.product_images.clone()) {
            Ok(value) => value,
            Err(error) => panic!("failed to map images: {error:?}"),
        };

        assert_eq!(Some("line"), structured.addressline.as_deref());
        assert_eq!(47.0, geo.lat);
        assert_eq!(Language::En, title.localization);
        assert_eq!(Language::De, description.localization);
        assert_eq!(1, images.len());
    }

    #[test]
    fn should_reject_incomplete_language_and_invalid_image_branches() {
        let mut row = product_row();
        row.title_language = None;
        assert!(matches!(
            localized_title_from_row(&row),
            Err(ProductRepositoryError::IncompleteTitlePersisted)
        ));

        row.title_language = Some("xx".to_owned());
        assert!(matches!(
            localized_title_from_row(&row),
            Err(ProductRepositoryError::InvalidTitleLanguagePersisted)
        ));

        let mut row = product_row();
        row.description_text = None;
        assert!(matches!(
            localized_description_from_row(&row),
            Err(ProductRepositoryError::IncompleteDescriptionPersisted)
        ));

        row.description_text = Some("description".to_owned());
        row.description_language = Some("xx".to_owned());
        assert!(matches!(
            localized_description_from_row(&row),
            Err(ProductRepositoryError::InvalidDescriptionLanguagePersisted)
        ));

        assert!(matches!(
            images_from_json(json!({"not": "array"})),
            Err(ProductRepositoryError::InvalidProductImagesPersisted)
        ));
        assert!(matches!(
            images_from_json(json!([{ "url": "not a url", "prohibited_content": "NONE" }])),
            Err(ProductRepositoryError::InvalidProductImageUrlPersisted)
        ));
        assert!(matches!(
            images_from_json(
                json!([{ "url": "https://example.com/a.jpg", "prohibited_content": "BAD" }])
            ),
            Err(ProductRepositoryError::InvalidProductImageProhibitedContentPersisted)
        ));
    }

    #[test]
    fn should_map_all_product_state_lifecycle_and_prohibited_content_values() {
        assert_eq!(ProductState::Listed, parse_state("LISTED"));
        assert_eq!(ProductState::Available, parse_state("AVAILABLE"));
        assert_eq!(ProductState::Reserved, parse_state("RESERVED"));
        assert_eq!(ProductState::Sold, parse_state("SOLD"));
        assert_eq!(ProductState::Removed, parse_state("REMOVED"));
        assert_eq!(ProductState::Unknown, parse_state("UNKNOWN"));
        assert_eq!("LISTED", product_state_as_str(ProductState::Listed));
        assert_eq!("AVAILABLE", product_state_as_str(ProductState::Available));
        assert_eq!("RESERVED", product_state_as_str(ProductState::Reserved));
        assert_eq!("SOLD", product_state_as_str(ProductState::Sold));
        assert_eq!("REMOVED", product_state_as_str(ProductState::Removed));
        assert_eq!("UNKNOWN", product_state_as_str(ProductState::Unknown));
        assert_eq!(ProductLifecycle::Active, parse_lifecycle("ACTIVE"));
        assert_eq!(ProductLifecycle::Deleted, parse_lifecycle("DELETED"));
        assert_eq!("ACTIVE", product_lifecycle_as_str(ProductLifecycle::Active));
        assert_eq!(
            "DELETED",
            product_lifecycle_as_str(ProductLifecycle::Deleted)
        );
        assert_eq!(ProhibitedContent::Unknown, parse_prohibited("UNKNOWN"));
        assert_eq!(ProhibitedContent::None, parse_prohibited("NONE"));
        assert_eq!(
            ProhibitedContent::NaziGermany,
            parse_prohibited("NAZI_GERMANY")
        );
    }

    #[test]
    fn should_reject_invalid_state_lifecycle_and_product_row_values() {
        assert!(matches!(
            parse_product_state("BAD"),
            Err(ProductRepositoryError::InvalidProductStatePersisted)
        ));
        assert!(matches!(
            parse_product_lifecycle("BAD"),
            Err(ProductRepositoryError::InvalidProductLifecyclePersisted)
        ));

        let mut row = product_row();
        row.url = "http://[::1".to_owned();
        assert!(matches!(
            Versioned::<Product, EventId>::try_from(row),
            Err(ProductRepositoryError::InvalidProductUrlPersisted)
        ));
    }

    fn parse_state(value: &str) -> ProductState {
        match parse_product_state(value) {
            Ok(state) => state,
            Err(error) => panic!("failed to parse product state: {error:?}"),
        }
    }

    fn parse_lifecycle(value: &str) -> ProductLifecycle {
        match parse_product_lifecycle(value) {
            Ok(lifecycle) => lifecycle,
            Err(error) => panic!("failed to parse lifecycle: {error:?}"),
        }
    }

    fn parse_prohibited(value: &str) -> ProhibitedContent {
        match parse_prohibited_content(value) {
            Ok(content) => content,
            Err(error) => panic!("failed to parse prohibited content: {error:?}"),
        }
    }

    fn product_row() -> ProductRow {
        let now = OffsetDateTime::now_utc();
        let slug = ProductSlugId::from("unit product").as_ref().to_owned();
        ProductRow {
            product_id: uuid::Uuid::new_v4(),
            product_slug_id: slug,
            event_id: uuid::Uuid::new_v4(),
            shop_id: uuid::Uuid::new_v4(),
            seller_id: uuid::Uuid::new_v4(),
            shops_product_id: "unit-product".to_owned(),
            structured_address_addressline: Some("line".to_owned()),
            structured_address_addressline_extra: Some("extra".to_owned()),
            structured_address_locality: Some("locality".to_owned()),
            structured_address_region: Some("region".to_owned()),
            structured_address_postal_code: Some("12345".to_owned()),
            structured_address_country: Some("DEU".to_owned()),
            geo_address_lat: Some(47.0),
            geo_address_lon: Some(8.0),
            title_text: Some("title".to_owned()),
            title_language: Some("en".to_owned()),
            description_text: Some("description".to_owned()),
            description_language: Some("de".to_owned()),
            price_amount: Some(1_200),
            price_currency: Some("EUR".to_owned()),
            price_estimate_min_amount: None,
            price_estimate_min_currency: None,
            price_estimate_max_amount: None,
            price_estimate_max_currency: None,
            sale_fx_rate_id: None,
            sold_at: None,
            state: "LISTED".to_owned(),
            lifecycle: "ACTIVE".to_owned(),
            url: "https://example.com/unit-product".to_owned(),
            product_images: json!([{ "url": "https://example.com/unit-product.jpg", "prohibited_content": "NONE" }]),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: now,
            updated: now,
        }
    }
}
