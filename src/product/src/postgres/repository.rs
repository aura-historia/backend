use crate::core::description::Description;
use crate::core::product::Product;
use crate::core::product_event::domain::ProductDomainEventPayload;
use crate::core::product_event::{ProductDomainEvent, ProductEvent, ProductEventPayload};
use crate::core::product_image::ProductImage;
use crate::core::prohibited_content::ProhibitedContent;
use crate::core::title::Title;
use crate::postgres::product_event_row::{ProductEventGroup, ProductEventRow};
use common::actor::domain::Actor;
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use geo::core::continent::Continent;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shop::core::shop_type::ShopType;
use sqlx::postgres::{PgQueryResult, PgRow};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use std::collections::HashMap;
use std::fmt::Display;

use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ProductPostgresRepositoryError {
    #[error("failed to map product Postgres row: {0}")]
    Mapping(String),
    #[error("failed to serialize product Postgres row")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to execute product Postgres query")]
    Sqlx(#[from] sqlx::Error),
    #[error("product write conflicted with current event id")]
    ConcurrentModification,
}

#[derive(Debug, Clone)]
pub struct ProductPostgresRepository {
    pool: PgPool,
}

impl ProductPostgresRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_product(
        &self,
        key: &ProductKey,
    ) -> Result<Option<Product>, ProductPostgresRepositoryError> {
        let sql = product_select_sql("WHERE shop_id = $1 AND shops_product_id = $2");
        let row = sqlx::query(&sql)
            .bind(uuid_from_display(key.shop_id)?)
            .bind(key.shops_product_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

        row.map(product_from_row).transpose()
    }

    pub async fn get_product_by_id(
        &self,
        product_id: ProductId,
    ) -> Result<Option<Product>, ProductPostgresRepositoryError> {
        let sql = product_select_sql("WHERE product_id = $1");
        let row = sqlx::query(&sql)
            .bind(uuid_from_display(product_id)?)
            .fetch_optional(&self.pool)
            .await?;

        row.map(product_from_row).transpose()
    }

    pub async fn get_event(
        &self,
        event_id: EventId,
    ) -> Result<Option<ProductEventRow>, ProductPostgresRepositoryError> {
        let row = sqlx::query(product_event_select_sql("WHERE event_id = $1"))
            .bind(uuid_from_display(event_id)?)
            .fetch_optional(&self.pool)
            .await?;

        row.map(product_event_from_row).transpose()
    }

    pub async fn list_events_for_product(
        &self,
        product_id: ProductId,
    ) -> Result<Vec<ProductEventRow>, ProductPostgresRepositoryError> {
        let rows = sqlx::query(
            "SELECT event_id, product_id, shop_id, shops_product_id, event_type, event_group, \
                    event_type_schema_version, payload, event_time, created_by \
             FROM product_events WHERE product_id = $1 ORDER BY event_time ASC, event_id ASC",
        )
        .bind(uuid_from_display(product_id)?)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(product_event_from_row).collect()
    }

    pub async fn insert_created_product(
        &self,
        event: ProductDomainEvent,
    ) -> Result<(), ProductPostgresRepositoryError> {
        let product = product_from_created_event(&event)?;
        let event = ProductEvent {
            aggregate_id: event.aggregate_id,
            event_id: event.event_id,
            timestamp: event.timestamp,
            payload: ProductEventPayload::ProductDomainEvent(event.payload),
        };
        self.insert_product_with_event(product, event).await
    }

    pub async fn insert_product_with_event(
        &self,
        product: Product,
        event: ProductEvent,
    ) -> Result<(), ProductPostgresRepositoryError> {
        let event_row = ProductEventRow::from_event(event);
        let mut tx = self.pool.begin().await?;

        insert_product(&mut tx, &product).await?;
        insert_product_event_row(&mut tx, &event_row).await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn update_product_with_events(
        &self,
        mut product: Product,
        events: Vec<ProductEvent>,
        expected_event_id: EventId,
    ) -> Result<(), ProductPostgresRepositoryError> {
        let Some(last_event) = events.last() else {
            return Ok(());
        };
        product.event_id = last_event.event_id;
        product.updated = last_event.timestamp;
        product.updated_by = Actor::System;

        let event_rows = events
            .into_iter()
            .map(ProductEventRow::from_event)
            .collect::<Vec<_>>();
        self.persist_product_events(product, event_rows, expected_event_id)
            .await
    }

    async fn persist_product_events(
        &self,
        product: Product,
        event_rows: Vec<ProductEventRow>,
        expected_event_id: EventId,
    ) -> Result<(), ProductPostgresRepositoryError> {
        if event_rows.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for event_row in &event_rows {
            insert_product_event_row(&mut tx, event_row).await?;
        }
        let update = update_product(&mut tx, &product, expected_event_id).await?;
        if update.rows_affected() != 1 {
            return Err(ProductPostgresRepositoryError::ConcurrentModification);
        }

        tx.commit().await?;
        Ok(())
    }
}

fn product_event_select_sql(where_clause: &str) -> &'static str {
    match where_clause {
        "WHERE event_id = $1" => {
            "SELECT event_id, product_id, shop_id, shops_product_id, event_type, event_group, \
                    event_type_schema_version, payload, event_time, created_by \
             FROM product_events WHERE event_id = $1"
        }
        _ => unreachable!("unsupported product event select"),
    }
}

async fn insert_product(
    tx: &mut Transaction<'_, Postgres>,
    product: &Product,
) -> Result<(), ProductPostgresRepositoryError> {
    let mut query = QueryBuilder::<Postgres>::new("INSERT INTO products (");
    push_product_columns(&mut query);
    query.push(") VALUES (");
    push_product_binds(&mut query, product)?;
    query.push(")");
    query.build().execute(&mut **tx).await?;
    Ok(())
}

async fn update_product(
    tx: &mut Transaction<'_, Postgres>,
    product: &Product,
    expected_event_id: EventId,
) -> Result<PgQueryResult, ProductPostgresRepositoryError> {
    let mut query = QueryBuilder::<Postgres>::new("UPDATE products SET (");
    push_product_columns(&mut query);
    query.push(") = (");
    push_product_binds(&mut query, product)?;
    query.push(") WHERE shop_id = ");
    query.push_bind(uuid_from_display(product.shop_id)?);
    query.push(" AND shops_product_id = ");
    query.push_bind(product.shops_product_id.to_string());
    query.push(" AND event_id = ");
    query.push_bind(uuid_from_display(expected_event_id)?);
    Ok(query.build().execute(&mut **tx).await?)
}

async fn insert_product_event_row(
    tx: &mut Transaction<'_, Postgres>,
    row: &ProductEventRow,
) -> Result<(), ProductPostgresRepositoryError> {
    sqlx::query(
        "INSERT INTO product_events (
            event_id, product_id, shop_id, shops_product_id, event_type, event_group,
            event_type_schema_version, payload, event_time, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(uuid_from_display(row.event_id)?)
    .bind(uuid_from_display(row.product_id)?)
    .bind(uuid_from_display(row.shop_id)?)
    .bind(row.shops_product_id.to_string())
    .bind(&row.event_type)
    .bind(row.event_group.as_str())
    .bind(row.event_type_schema_version)
    .bind(sqlx::types::Json(&row.payload))
    .bind(row.event_time)
    .bind(String::from(row.created_by))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn push_product_columns(query: &mut QueryBuilder<'_, Postgres>) {
    let mut columns = query.separated(", ");
    for column in PRODUCT_COLUMNS {
        columns.push(*column);
    }
}

fn push_product_binds<'a>(
    query: &mut QueryBuilder<'a, Postgres>,
    product: &'a Product,
) -> Result<(), ProductPostgresRepositoryError> {
    let images = product_images_json(product);
    let created_by = String::from(product.created_by);
    let updated_by = String::from(product.updated_by);

    let mut values = query.separated(", ");
    values.push_bind(uuid_from_display(product.product_id)?);
    values.push_bind(product.product_slug_id.to_string());
    values.push_bind(product.shop_slug_id.to_string());
    values.push_bind(product.seller_slug_id.to_string());
    values.push_bind(uuid_from_display(product.event_id)?);
    values.push_bind(uuid_from_display(product.shop_id)?);
    values.push_bind(uuid_from_display(product.seller_id)?);
    values.push_bind(product.shops_product_id.to_string());
    values.push_bind(product.shop_name.to_string());
    values.push_bind(product.seller_name.to_string());
    values.push_bind(shop_type_db(product.shop_type));
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.addressline.as_deref()),
    );
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.addressline_extra.as_deref()),
    );
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.locality.as_deref()),
    );
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.region.as_deref()),
    );
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.postal_code.as_deref()),
    );
    values.push_bind(
        product
            .structured_address
            .as_ref()
            .and_then(|value| value.country.map(|country| country.alpha2())),
    );
    values.push_bind(product.geo_address.map(|value| value.lat));
    values.push_bind(product.geo_address.map(|value| value.lon));
    values.push_bind(product.native_title.payload.as_ref());
    values.push_bind(product.native_title.localization.as_str());
    values.push_bind(product.other_title.get(&Language::De).map(AsRef::as_ref));
    values.push_bind(product.other_title.get(&Language::En).map(AsRef::as_ref));
    values.push_bind(product.other_title.get(&Language::Fr).map(AsRef::as_ref));
    values.push_bind(product.other_title.get(&Language::Es).map(AsRef::as_ref));
    values.push_bind(product.other_title.get(&Language::It).map(AsRef::as_ref));
    values.push_bind(
        product
            .native_description
            .as_ref()
            .map(|value| value.payload.as_ref()),
    );
    values.push_bind(
        product
            .native_description
            .as_ref()
            .map(|value| value.localization.as_str()),
    );
    push_price_binds(&mut values, &product.native_price, &product.other_price)?;
    push_price_binds(
        &mut values,
        &product.native_price_estimate_min,
        &product.other_price_estimate_min,
    )?;
    push_price_binds(
        &mut values,
        &product.native_price_estimate_max,
        &product.other_price_estimate_max,
    )?;
    values.push_bind(product_state_db(product.state));
    values.push_bind(product_lifecycle_db(product.lifecycle));
    values.push_bind(product.url.as_str());
    values.push_bind(product.view_url.as_str());
    values.push_bind(sqlx::types::Json(images));
    values.push_bind(product.embedding.clone());
    values.push_bind(product.auction_start);
    values.push_bind(product.auction_end);
    values.push_bind(created_by);
    values.push_bind(updated_by);
    values.push_bind(product.created);
    values.push_bind(product.updated);
    Ok(())
}

fn push_price_binds<'a>(
    values: &mut sqlx::query_builder::Separated<'_, 'a, Postgres, &'static str>,
    native: &'a Option<Price>,
    other: &'a HashMap<Currency, MonetaryAmount>,
) -> Result<(), ProductPostgresRepositoryError> {
    values.push_bind(
        native
            .map(|price| i64_from_u64(u64::from(price.monetary_amount)))
            .transpose()?,
    );
    values.push_bind(native.map(|price| price.currency.as_str()));
    for currency in PRICE_COLUMNS {
        values.push_bind(
            other
                .get(currency)
                .copied()
                .map(u64::from)
                .map(i64_from_u64)
                .transpose()?,
        );
    }
    Ok(())
}

fn product_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT {} FROM products {}",
        PRODUCT_COLUMNS.join(", "),
        where_clause
    )
}

fn product_from_created_event(
    event: &ProductDomainEvent,
) -> Result<Product, ProductPostgresRepositoryError> {
    let ProductDomainEventPayload::Created(payload) = event.payload.clone() else {
        return Err(ProductPostgresRepositoryError::Mapping(
            "created product insert needs DOMAIN_CREATED event".to_owned(),
        ));
    };

    Ok(Product {
        product_id: event.aggregate_id,
        product_slug_id: payload.product_slug_id,
        shop_slug_id: payload.shop_slug_id,
        seller_slug_id: payload.seller_slug_id,
        event_id: event.event_id,
        shop_id: payload.shop_id,
        seller_id: payload.seller_id,
        shops_product_id: payload.shops_product_id,
        shop_name: payload.shop_name,
        seller_name: payload.seller_name,
        shop_type: payload.shop_type,
        structured_address: payload.structured_address,
        geo_address: payload.geo_address,
        native_title: payload.native_title,
        other_title: HashMap::new(),
        native_description: payload.native_description,
        native_price: payload.native_price,
        other_price: payload.other_price,
        native_price_estimate_min: payload.native_price_estimate_min,
        other_price_estimate_min: payload.other_price_estimate_min,
        native_price_estimate_max: payload.native_price_estimate_max,
        other_price_estimate_max: payload.other_price_estimate_max,
        state: payload.state,
        lifecycle: ProductLifecycle::Active,
        url: payload.url,
        view_url: payload.view_url,
        images: payload.images,
        embedding: None,
        auction_start: payload.auction_start,
        auction_end: payload.auction_end,
        created_by: Actor::System,
        updated_by: Actor::System,
        created: event.timestamp,
        updated: event.timestamp,
    })
}

fn product_from_row(row: PgRow) -> Result<Product, ProductPostgresRepositoryError> {
    let product_id: ProductId = row.get::<Uuid, _>("product_id").into();
    let shop_id: ShopId = row.get::<Uuid, _>("shop_id").into();
    let seller_id: ShopId = row.get::<Uuid, _>("seller_id").into();
    let images: sqlx::types::Json<Value> = row.get("product_images");

    Ok(Product {
        product_id,
        product_slug_id: ProductSlugId::from(row.get::<String, _>("product_slug_id")),
        shop_slug_id: ShopSlugId::from(row.get::<String, _>("shop_slug_id")),
        seller_slug_id: SellerSlugId::from(row.get::<String, _>("seller_slug_id")),
        event_id: row.get::<Uuid, _>("event_id").into(),
        shop_id,
        seller_id,
        shops_product_id: ShopsProductId::from(row.get::<String, _>("shops_product_id")),
        shop_name: ShopName::from(row.get::<String, _>("shop_name")),
        seller_name: ShopName::from(row.get::<String, _>("seller_name")),
        shop_type: shop_type_from_db(row.get::<String, _>("shop_type"))?,
        structured_address: structured_address_from_row(&row)?,
        geo_address: geo_address_from_row(&row)?,
        native_title: Localized::new(
            language_from_db(row.get::<String, _>("title_native_language"))?,
            Title::from(row.get::<String, _>("title_native_text")),
        ),
        other_title: other_titles_from_row(&row),
        native_description: description_from_row(&row)?,
        native_price: price_from_row(&row, "price_native_amount", "price_native_currency")?,
        other_price: money_map_from_row(&row, "price")?,
        native_price_estimate_min: price_from_row(
            &row,
            "price_estimate_min_native_amount",
            "price_estimate_min_native_currency",
        )?,
        other_price_estimate_min: money_map_from_row(&row, "price_estimate_min")?,
        native_price_estimate_max: price_from_row(
            &row,
            "price_estimate_max_native_amount",
            "price_estimate_max_native_currency",
        )?,
        other_price_estimate_max: money_map_from_row(&row, "price_estimate_max")?,
        state: product_state_from_db(row.get::<String, _>("state"))?,
        lifecycle: product_lifecycle_from_db(row.get::<String, _>("lifecycle"))?,
        url: parse_url(row.get::<String, _>("url"), "product url")?,
        view_url: parse_url(row.get::<String, _>("view_url"), "product view_url")?,
        images: product_images_from_json(images.0)?,
        embedding: row.get("embedding"),
        auction_start: row.get("auction_start"),
        auction_end: row.get("auction_end"),
        created_by: Actor::try_from(row.get::<String, _>("created_by"))
            .map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))?,
        updated_by: Actor::try_from(row.get::<String, _>("updated_by"))
            .map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))?,
        created: row.get("created"),
        updated: row.get("updated"),
    })
}

fn product_event_from_row(row: PgRow) -> Result<ProductEventRow, ProductPostgresRepositoryError> {
    Ok(ProductEventRow {
        event_id: row.get::<Uuid, _>("event_id").into(),
        product_id: row.get::<Uuid, _>("product_id").into(),
        shop_id: row.get::<Uuid, _>("shop_id").into(),
        shops_product_id: ShopsProductId::from(row.get::<String, _>("shops_product_id")),
        event_type: row.get("event_type"),
        event_group: product_event_group_from_db(row.get::<String, _>("event_group"))?,
        event_type_schema_version: row.get("event_type_schema_version"),
        payload: row.get::<sqlx::types::Json<Value>, _>("payload").0,
        event_time: row.get("event_time"),
        created_by: Actor::try_from(row.get::<String, _>("created_by"))
            .map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))?,
    })
}

fn structured_address_from_row(
    row: &PgRow,
) -> Result<Option<StructuredAddress>, ProductPostgresRepositoryError> {
    let country = row
        .get::<Option<String>, _>("structured_address_country")
        .map(|country| {
            isocountry::CountryCode::for_alpha2(&country).map_err(|err| {
                ProductPostgresRepositoryError::Mapping(format!("invalid country code: {err}"))
            })
        })
        .transpose()?;
    let address = StructuredAddress {
        addressline: row.get("structured_address_addressline"),
        addressline_extra: row.get("structured_address_addressline_extra"),
        locality: row.get("structured_address_locality"),
        region: row.get("structured_address_region"),
        postal_code: row.get("structured_address_postal_code"),
        country,
        continent: country.map(Continent::from),
    };
    Ok((!address.is_empty()).then_some(address))
}

fn geo_address_from_row(row: &PgRow) -> Result<Option<GeoAddress>, ProductPostgresRepositoryError> {
    match (
        row.get::<Option<f64>, _>("geo_address_lat"),
        row.get::<Option<f64>, _>("geo_address_lon"),
    ) {
        (Some(lat), Some(lon)) => Ok(Some(GeoAddress { lat, lon })),
        (None, None) => Ok(None),
        _ => Err(ProductPostgresRepositoryError::Mapping(
            "geo lat and lon must both be present".to_owned(),
        )),
    }
}

fn other_titles_from_row(row: &PgRow) -> HashMap<Language, Title> {
    [
        (Language::De, row.get::<Option<String>, _>("title_de")),
        (Language::En, row.get::<Option<String>, _>("title_en")),
        (Language::Fr, row.get::<Option<String>, _>("title_fr")),
        (Language::Es, row.get::<Option<String>, _>("title_es")),
        (Language::It, row.get::<Option<String>, _>("title_it")),
    ]
    .into_iter()
    .filter_map(|(language, title)| title.map(|title| (language, Title::from(title))))
    .collect()
}

fn description_from_row(
    row: &PgRow,
) -> Result<Option<Localized<Language, Description>>, ProductPostgresRepositoryError> {
    match (
        row.get::<Option<String>, _>("description_native_text"),
        row.get::<Option<String>, _>("description_native_language"),
    ) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            language_from_db(language)?,
            Description::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductPostgresRepositoryError::Mapping(
            "description text and language must both be present".to_owned(),
        )),
    }
}

fn price_from_row(
    row: &PgRow,
    amount_column: &str,
    currency_column: &str,
) -> Result<Option<Price>, ProductPostgresRepositoryError> {
    match (
        row.get::<Option<i64>, _>(amount_column),
        row.get::<Option<String>, _>(currency_column),
    ) {
        (Some(amount), Some(currency)) => Ok(Some(Price::new(
            u64_from_i64(amount)?.into(),
            currency_from_db(currency)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductPostgresRepositoryError::Mapping(format!(
            "{amount_column} and {currency_column} must both be present"
        ))),
    }
}

fn money_map_from_row(
    row: &PgRow,
    prefix: &str,
) -> Result<HashMap<Currency, MonetaryAmount>, ProductPostgresRepositoryError> {
    let mut result = HashMap::new();
    for currency in PRICE_COLUMNS {
        let column = format!("{}_{}", prefix, currency.as_str().to_ascii_lowercase());
        if let Some(amount) = row.get::<Option<i64>, _>(column.as_str()) {
            result.insert(*currency, u64_from_i64(amount)?.into());
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProductImageJson {
    url: String,
    prohibited_content: String,
}

fn product_images_json(product: &Product) -> Vec<ProductImageJson> {
    product
        .images
        .iter()
        .map(|image| ProductImageJson {
            url: image.url.to_string(),
            prohibited_content: image.prohibited_content.as_str().to_owned(),
        })
        .collect()
}

fn product_images_from_json(
    value: Value,
) -> Result<IndexSet<ProductImage>, ProductPostgresRepositoryError> {
    let images: Vec<ProductImageJson> = serde_json::from_value(value)?;
    images
        .into_iter()
        .map(|image| {
            Ok(ProductImage {
                url: parse_url(image.url, "product image url")?,
                prohibited_content: prohibited_content_from_db(image.prohibited_content)?,
            })
        })
        .collect()
}

fn parse_url(value: String, field: &str) -> Result<url::Url, ProductPostgresRepositoryError> {
    value
        .parse()
        .map_err(|err| ProductPostgresRepositoryError::Mapping(format!("invalid {field}: {err}")))
}

fn shop_type_db(value: ShopType) -> &'static str {
    match value {
        ShopType::AuctionHouse => "AUCTION_HOUSE",
        ShopType::AuctionPlatform => "AUCTION_PLATFORM",
        ShopType::CommercialDealer => "COMMERCIAL_DEALER",
        ShopType::Marketplace => "MARKETPLACE",
    }
}

fn shop_type_from_db(value: String) -> Result<ShopType, ProductPostgresRepositoryError> {
    match value.as_str() {
        "AUCTION_HOUSE" => Ok(ShopType::AuctionHouse),
        "AUCTION_PLATFORM" => Ok(ShopType::AuctionPlatform),
        "COMMERCIAL_DEALER" => Ok(ShopType::CommercialDealer),
        "MARKETPLACE" => Ok(ShopType::Marketplace),
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown shop type: {other}"
        ))),
    }
}

fn product_state_db(value: ProductState) -> &'static str {
    match value {
        ProductState::Listed => "LISTED",
        ProductState::Available => "AVAILABLE",
        ProductState::Reserved => "RESERVED",
        ProductState::Sold => "SOLD",
        ProductState::Removed => "REMOVED",
        ProductState::Unknown => "UNKNOWN",
    }
}

fn product_state_from_db(value: String) -> Result<ProductState, ProductPostgresRepositoryError> {
    match value.as_str() {
        "LISTED" => Ok(ProductState::Listed),
        "AVAILABLE" => Ok(ProductState::Available),
        "RESERVED" => Ok(ProductState::Reserved),
        "SOLD" => Ok(ProductState::Sold),
        "REMOVED" => Ok(ProductState::Removed),
        "UNKNOWN" => Ok(ProductState::Unknown),
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown product state: {other}"
        ))),
    }
}

fn product_lifecycle_db(value: ProductLifecycle) -> &'static str {
    match value {
        ProductLifecycle::Active => "ACTIVE",
        ProductLifecycle::Deleted => "DELETED",
    }
}

fn product_lifecycle_from_db(
    value: String,
) -> Result<ProductLifecycle, ProductPostgresRepositoryError> {
    match value.as_str() {
        "ACTIVE" => Ok(ProductLifecycle::Active),
        "DELETED" => Ok(ProductLifecycle::Deleted),
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown product lifecycle: {other}"
        ))),
    }
}

fn product_event_group_from_db(
    value: String,
) -> Result<ProductEventGroup, ProductPostgresRepositoryError> {
    match value.as_str() {
        "DOMAIN" => Ok(ProductEventGroup::Domain),
        "ENRICHMENT" => Ok(ProductEventGroup::Enrichment),
        "POLICY" => Ok(ProductEventGroup::Policy),
        "LIFECYCLE" => Ok(ProductEventGroup::Lifecycle),
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown product event group: {other}"
        ))),
    }
}

fn language_from_db(value: String) -> Result<Language, ProductPostgresRepositoryError> {
    match value.as_str() {
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
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown language: {other}"
        ))),
    }
}

fn currency_from_db(value: String) -> Result<Currency, ProductPostgresRepositoryError> {
    match value.as_str() {
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
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown currency: {other}"
        ))),
    }
}

fn prohibited_content_from_db(
    value: String,
) -> Result<ProhibitedContent, ProductPostgresRepositoryError> {
    match value.as_str() {
        "UNKNOWN" => Ok(ProhibitedContent::Unknown),
        "NONE" => Ok(ProhibitedContent::None),
        "NAZI_GERMANY" => Ok(ProhibitedContent::NaziGermany),
        other => Err(ProductPostgresRepositoryError::Mapping(format!(
            "unknown prohibited content: {other}"
        ))),
    }
}

fn uuid_from_display(value: impl Display) -> Result<Uuid, ProductPostgresRepositoryError> {
    Uuid::parse_str(&value.to_string())
        .map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))
}

fn i64_from_u64(value: u64) -> Result<i64, ProductPostgresRepositoryError> {
    i64::try_from(value).map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))
}

fn u64_from_i64(value: i64) -> Result<u64, ProductPostgresRepositoryError> {
    u64::try_from(value).map_err(|err| ProductPostgresRepositoryError::Mapping(err.to_string()))
}

const PRICE_COLUMNS: &[Currency] = &[
    Currency::Eur,
    Currency::Usd,
    Currency::Gbp,
    Currency::Aud,
    Currency::Cad,
    Currency::Nzd,
    Currency::Cny,
    Currency::Brl,
    Currency::Pln,
    Currency::Try,
    Currency::Jpy,
    Currency::Czk,
    Currency::Rub,
    Currency::Aed,
    Currency::Sar,
    Currency::Hkd,
    Currency::Sgd,
    Currency::Chf,
];

const PRODUCT_COLUMNS: &[&str] = &[
    "product_id",
    "product_slug_id",
    "shop_slug_id",
    "seller_slug_id",
    "event_id",
    "shop_id",
    "seller_id",
    "shops_product_id",
    "shop_name",
    "seller_name",
    "shop_type",
    "structured_address_addressline",
    "structured_address_addressline_extra",
    "structured_address_locality",
    "structured_address_region",
    "structured_address_postal_code",
    "structured_address_country",
    "geo_address_lat",
    "geo_address_lon",
    "title_native_text",
    "title_native_language",
    "title_de",
    "title_en",
    "title_fr",
    "title_es",
    "title_it",
    "description_native_text",
    "description_native_language",
    "price_native_amount",
    "price_native_currency",
    "price_eur",
    "price_usd",
    "price_gbp",
    "price_aud",
    "price_cad",
    "price_nzd",
    "price_cny",
    "price_brl",
    "price_pln",
    "price_try",
    "price_jpy",
    "price_czk",
    "price_rub",
    "price_aed",
    "price_sar",
    "price_hkd",
    "price_sgd",
    "price_chf",
    "price_estimate_min_native_amount",
    "price_estimate_min_native_currency",
    "price_estimate_min_eur",
    "price_estimate_min_usd",
    "price_estimate_min_gbp",
    "price_estimate_min_aud",
    "price_estimate_min_cad",
    "price_estimate_min_nzd",
    "price_estimate_min_cny",
    "price_estimate_min_brl",
    "price_estimate_min_pln",
    "price_estimate_min_try",
    "price_estimate_min_jpy",
    "price_estimate_min_czk",
    "price_estimate_min_rub",
    "price_estimate_min_aed",
    "price_estimate_min_sar",
    "price_estimate_min_hkd",
    "price_estimate_min_sgd",
    "price_estimate_min_chf",
    "price_estimate_max_native_amount",
    "price_estimate_max_native_currency",
    "price_estimate_max_eur",
    "price_estimate_max_usd",
    "price_estimate_max_gbp",
    "price_estimate_max_aud",
    "price_estimate_max_cad",
    "price_estimate_max_nzd",
    "price_estimate_max_cny",
    "price_estimate_max_brl",
    "price_estimate_max_pln",
    "price_estimate_max_try",
    "price_estimate_max_jpy",
    "price_estimate_max_czk",
    "price_estimate_max_rub",
    "price_estimate_max_aed",
    "price_estimate_max_sar",
    "price_estimate_max_hkd",
    "price_estimate_max_sgd",
    "price_estimate_max_chf",
    "state",
    "lifecycle",
    "url",
    "view_url",
    "product_images",
    "embedding",
    "auction_start",
    "auction_end",
    "created_by",
    "updated_by",
    "created",
    "updated",
];
