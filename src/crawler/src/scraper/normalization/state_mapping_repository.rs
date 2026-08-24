use crate::scraper::normalization::state::{ProductStateMappingRecord, StateMappingType};
use product_listing_core::product_state::ProductState;
use sqlx::{PgPool, Row};

use time::OffsetDateTime;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductStateMappingRepository {
    async fn find_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ProductStateMappingRecord>, sqlx::Error>;

    /// Returns every mapping whose `mapping_type` is `REGEX`.
    /// The caller is responsible for compiling and applying the patterns.
    async fn find_all_regex_mappings(&self) -> Result<Vec<ProductStateMappingRecord>, sqlx::Error>;

    async fn insert_mapping(
        &self,
        record: &ProductStateMappingRecord,
    ) -> Result<ProductStateMappingRecord, sqlx::Error>;

    async fn update_mapping(
        &self,
        raw: &str,
        normalized: &ProductState,
        mapping_type: &StateMappingType,
    ) -> Result<ProductStateMappingRecord, sqlx::Error>;
}

pub struct ProductStateMappingRepositoryImpl<'a> {
    pool: &'a PgPool,
}

impl<'a> ProductStateMappingRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn product_state_from_db_str(value: &str) -> Result<ProductState, sqlx::Error> {
    ProductState::from_code(value).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unknown ProductState: {value}"),
        )))
    })
}

fn mapping_type_to_db_str(t: &StateMappingType) -> &'static str {
    match t {
        StateMappingType::Value => "VALUE",
        StateMappingType::Regex => "REGEX",
    }
}

fn mapping_type_from_db_str(s: &str) -> Result<StateMappingType, sqlx::Error> {
    match s {
        "VALUE" => Ok(StateMappingType::Value),
        "REGEX" => Ok(StateMappingType::Regex),
        other => Err(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unknown StateMappingType: {other}"),
        )))),
    }
}

fn row_to_record(row: sqlx::postgres::PgRow) -> Result<ProductStateMappingRecord, sqlx::Error> {
    let raw: String = row.try_get("raw")?;
    let normalized_str: String = row.try_get("normalized")?;
    let mapping_type_str: String = row.try_get("mapping_type")?;
    let created: OffsetDateTime = row.try_get("created")?;
    let updated: OffsetDateTime = row.try_get("updated")?;

    let normalized = product_state_from_db_str(&normalized_str)?;
    let mapping_type = mapping_type_from_db_str(&mapping_type_str)?;

    Ok(ProductStateMappingRecord {
        raw,
        normalized,
        mapping_type,
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl<'a> ProductStateMappingRepository for ProductStateMappingRepositoryImpl<'a> {
    async fn find_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ProductStateMappingRecord>, sqlx::Error> {
        sqlx::query(
            "SELECT raw, normalized, mapping_type, created, updated
             FROM product_state_mapping
             WHERE raw = $1",
        )
        .bind(raw)
        .fetch_optional(self.pool)
        .await?
        .map(row_to_record)
        .transpose()
    }

    async fn insert_mapping(
        &self,
        record: &ProductStateMappingRecord,
    ) -> Result<ProductStateMappingRecord, sqlx::Error> {
        sqlx::query(
            "INSERT INTO product_state_mapping (raw, normalized, mapping_type, created, updated)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING raw, normalized, mapping_type, created, updated",
        )
        .bind(&record.raw)
        .bind(record.normalized.as_str())
        .bind(mapping_type_to_db_str(&record.mapping_type))
        .bind(record.created)
        .bind(record.updated)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_record)
    }

    async fn find_all_regex_mappings(&self) -> Result<Vec<ProductStateMappingRecord>, sqlx::Error> {
        sqlx::query(
            "SELECT raw, normalized, mapping_type, created, updated
             FROM product_state_mapping
             WHERE mapping_type = 'REGEX'",
        )
        .fetch_all(self.pool)
        .await?
        .into_iter()
        .map(row_to_record)
        .collect()
    }

    async fn update_mapping(
        &self,
        raw: &str,
        normalized: &ProductState,
        mapping_type: &StateMappingType,
    ) -> Result<ProductStateMappingRecord, sqlx::Error> {
        sqlx::query(
            "UPDATE product_state_mapping
             SET normalized = $2, mapping_type = $3, updated = NOW()
             WHERE raw = $1
             RETURNING raw, normalized, mapping_type, created, updated",
        )
        .bind(raw)
        .bind(normalized.as_str())
        .bind(mapping_type_to_db_str(mapping_type))
        .fetch_one(self.pool)
        .await
        .and_then(row_to_record)
    }
}
