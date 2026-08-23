use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, ShopsProductSchema};
use shop_core::shop_id::ShopId;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopsProductSchemaRepository {
    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, sqlx::Error>;

    async fn insert_product_schema(
        &self,
        shop_id: &ShopId,
        schema: &ShopsProductSchema,
    ) -> Result<ShopsProductSchema, sqlx::Error>;

    async fn update_product_schema(
        &self,
        shop_id: &ShopId,
        product_schemas: &[ProductCssSelectorSchema],
    ) -> Result<ShopsProductSchema, sqlx::Error>;
}

pub struct ShopsProductSchemaRepositoryImpl<'a> {
    pool: &'a PgPool,
}

impl<'a> ShopsProductSchemaRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_schema(row: sqlx::postgres::PgRow) -> Result<ShopsProductSchema, sqlx::Error> {
    let shop_id_uuid: Uuid = row.try_get("shop_id")?;
    let product_schema_json: serde_json::Value = row.try_get("product_schema")?;
    let created: OffsetDateTime = row.try_get("created")?;
    let updated: OffsetDateTime = row.try_get("updated")?;

    let product_schemas: Vec<ProductCssSelectorSchema> = match serde_json::from_value::<
        Vec<ProductCssSelectorSchema>,
    >(product_schema_json.clone())
    {
        Ok(list) => list,
        Err(_) => vec![
            serde_json::from_value::<ProductCssSelectorSchema>(product_schema_json)
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        ],
    };
    Ok(ShopsProductSchema {
        shop_id: ShopId::from(shop_id_uuid),
        product_schemas,
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl<'a> ShopsProductSchemaRepository for ShopsProductSchemaRepositoryImpl<'a> {
    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, sqlx::Error> {
        sqlx::query(
            "SELECT shop_id, product_schema, created, updated
             FROM shops_product_schema
             WHERE shop_id = $1",
        )
        .bind(Uuid::from(*shop_id))
        .fetch_optional(self.pool)
        .await?
        .map(row_to_schema)
        .transpose()
    }

    async fn insert_product_schema(
        &self,
        shop_id: &ShopId,
        schema: &ShopsProductSchema,
    ) -> Result<ShopsProductSchema, sqlx::Error> {
        let schemas = schema.product_schemas.clone();
        let product_schema_json =
            serde_json::to_value(&schemas).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "INSERT INTO shops_product_schema (shop_id, product_schema, created, updated)
             VALUES ($1, $2, $3, $4)
             RETURNING shop_id, product_schema, created, updated",
        )
        .bind(Uuid::from(*shop_id))
        .bind(product_schema_json)
        .bind(schema.created)
        .bind(schema.updated)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }

    async fn update_product_schema(
        &self,
        shop_id: &ShopId,
        product_schemas: &[ProductCssSelectorSchema],
    ) -> Result<ShopsProductSchema, sqlx::Error> {
        let product_schema_json =
            serde_json::to_value(product_schemas).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "UPDATE shops_product_schema
             SET product_schema = $2, updated = NOW()
             WHERE shop_id = $1
             RETURNING shop_id, product_schema, created, updated",
        )
        .bind(Uuid::from(*shop_id))
        .bind(product_schema_json)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }
}
