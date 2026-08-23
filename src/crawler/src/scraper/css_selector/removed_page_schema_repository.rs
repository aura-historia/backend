use crate::scraper::css_selector::removed_page_schema::{
    RemovedPageSchema, ShopsRemovedPageSchema,
};
use shop_core::shop_id::ShopId;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[async_trait::async_trait]
#[mockall::automock]
pub trait RemovedPageSchemaRepository {
    async fn find_removed_page_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsRemovedPageSchema>, sqlx::Error>;

    async fn insert_removed_page_schema(
        &self,
        shop_id: &ShopId,
        schema: &ShopsRemovedPageSchema,
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error>;

    async fn update_removed_page_schema(
        &self,
        shop_id: &ShopId,
        removed_page_schemas: &[RemovedPageSchema],
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error>;
}

/// Disabled persistence fallback used by tests and simple constructors.
///
/// Reads act as an empty schema cache. Writes fail because this repository does not own durable
/// storage; production wires [`RemovedPageSchemaRepositoryImpl`] explicitly.
pub struct NullRemovedPageSchemaRepository;

#[async_trait::async_trait]
impl RemovedPageSchemaRepository for NullRemovedPageSchemaRepository {
    async fn find_removed_page_schema(
        &self,
        _: &ShopId,
    ) -> Result<Option<ShopsRemovedPageSchema>, sqlx::Error> {
        Ok(None)
    }

    async fn insert_removed_page_schema(
        &self,
        _: &ShopId,
        _: &ShopsRemovedPageSchema,
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }

    async fn update_removed_page_schema(
        &self,
        _: &ShopId,
        _: &[RemovedPageSchema],
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }
}

pub struct RemovedPageSchemaRepositoryImpl<'a> {
    pool: &'a PgPool,
}

impl<'a> RemovedPageSchemaRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_schema(row: sqlx::postgres::PgRow) -> Result<ShopsRemovedPageSchema, sqlx::Error> {
    let shop_id_uuid: Uuid = row.try_get("shop_id")?;
    let schema_json: serde_json::Value = row.try_get("removed_page_schema")?;
    let created: OffsetDateTime = row.try_get("created")?;
    let updated: OffsetDateTime = row.try_get("updated")?;

    let removed_page_schemas: Vec<RemovedPageSchema> =
        serde_json::from_value(schema_json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    Ok(ShopsRemovedPageSchema {
        shop_id: ShopId::from(shop_id_uuid),
        removed_page_schemas,
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl<'a> RemovedPageSchemaRepository for RemovedPageSchemaRepositoryImpl<'a> {
    async fn find_removed_page_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsRemovedPageSchema>, sqlx::Error> {
        sqlx::query(
            "SELECT shop_id, removed_page_schema, created, updated
             FROM shops_removed_page_schema
             WHERE shop_id = $1",
        )
        .bind(Uuid::from(*shop_id))
        .fetch_optional(self.pool)
        .await?
        .map(row_to_schema)
        .transpose()
    }

    async fn insert_removed_page_schema(
        &self,
        shop_id: &ShopId,
        schema: &ShopsRemovedPageSchema,
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error> {
        let schema_json = serde_json::to_value(&schema.removed_page_schemas)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "INSERT INTO shops_removed_page_schema (shop_id, removed_page_schema, created, updated)
             VALUES ($1, $2, $3, $4)
             RETURNING shop_id, removed_page_schema, created, updated",
        )
        .bind(Uuid::from(*shop_id))
        .bind(schema_json)
        .bind(schema.created)
        .bind(schema.updated)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }

    async fn update_removed_page_schema(
        &self,
        shop_id: &ShopId,
        removed_page_schemas: &[RemovedPageSchema],
    ) -> Result<ShopsRemovedPageSchema, sqlx::Error> {
        let schema_json = serde_json::to_value(removed_page_schemas)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "UPDATE shops_removed_page_schema
             SET removed_page_schema = $2, updated = NOW()
             WHERE shop_id = $1
             RETURNING shop_id, removed_page_schema, created, updated",
        )
        .bind(Uuid::from(*shop_id))
        .bind(schema_json)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }
}
