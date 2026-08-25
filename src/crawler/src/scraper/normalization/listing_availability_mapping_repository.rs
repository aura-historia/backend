use crate::scraper::normalization::listing_availability_mapping::{
    ListingAvailabilityDecisionKind, ListingAvailabilityMappingRecord,
    ListingAvailabilityMappingType,
};
use product_listing_core::listing_availability::ListingAvailability;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ListingAvailabilityMappingRepository {
    async fn find_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ListingAvailabilityMappingRecord>, sqlx::Error>;
    async fn find_all_regex_mappings(
        &self,
    ) -> Result<Vec<ListingAvailabilityMappingRecord>, sqlx::Error>;
    async fn insert_mapping(
        &self,
        record: &ListingAvailabilityMappingRecord,
    ) -> Result<ListingAvailabilityMappingRecord, sqlx::Error>;
    async fn update_mapping(
        &self,
        record: &ListingAvailabilityMappingRecord,
    ) -> Result<ListingAvailabilityMappingRecord, sqlx::Error>;
}

pub struct ListingAvailabilityMappingRepositoryImpl<'a> {
    pool: &'a PgPool,
}
impl<'a> ListingAvailabilityMappingRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn decode(value: Option<&str>) -> Result<Option<ListingAvailability>, sqlx::Error> {
    value
        .map(|value| {
            ListingAvailability::from_code(value).ok_or_else(|| {
                sqlx::Error::Decode(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid crawler availability: {value}"),
                )))
            })
        })
        .transpose()
}
fn mapping_type(value: &str) -> Result<ListingAvailabilityMappingType, sqlx::Error> {
    match value {
        "VALUE" => Ok(ListingAvailabilityMappingType::Value),
        "REGEX" => Ok(ListingAvailabilityMappingType::Regex),
        _ => Err(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid crawler availability mapping type",
        )))),
    }
}
fn decision_kind(value: &str) -> Result<ListingAvailabilityDecisionKind, sqlx::Error> {
    match value {
        "AVAILABILITY" => Ok(ListingAvailabilityDecisionKind::Availability),
        "NO_ASSERTION" => Ok(ListingAvailabilityDecisionKind::NoAssertion),
        _ => Err(sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid crawler availability decision kind",
        )))),
    }
}
fn row(row: sqlx::postgres::PgRow) -> Result<ListingAvailabilityMappingRecord, sqlx::Error> {
    let availability: Option<String> = row.try_get("availability")?;
    Ok(ListingAvailabilityMappingRecord {
        raw: row.try_get("raw")?,
        availability: decode(availability.as_deref())?,
        mapping_type: mapping_type(&row.try_get::<String, _>("mapping_type")?)?,
        decision_kind: decision_kind(&row.try_get::<String, _>("decision_kind")?)?,
        created: row.try_get::<OffsetDateTime, _>("created")?,
        updated: row.try_get::<OffsetDateTime, _>("updated")?,
    })
}

#[async_trait::async_trait]
impl<'a> ListingAvailabilityMappingRepository for ListingAvailabilityMappingRepositoryImpl<'a> {
    async fn find_mapping(
        &self,
        raw: &str,
    ) -> Result<Option<ListingAvailabilityMappingRecord>, sqlx::Error> {
        sqlx::query("SELECT raw, availability, mapping_type, decision_kind, created, updated FROM listing_availability_mapping WHERE raw = $1").bind(raw).fetch_optional(self.pool).await?.map(row).transpose()
    }
    async fn find_all_regex_mappings(
        &self,
    ) -> Result<Vec<ListingAvailabilityMappingRecord>, sqlx::Error> {
        sqlx::query("SELECT raw, availability, mapping_type, decision_kind, created, updated FROM listing_availability_mapping WHERE mapping_type = 'REGEX'").fetch_all(self.pool).await?.into_iter().map(row).collect()
    }
    async fn insert_mapping(
        &self,
        record: &ListingAvailabilityMappingRecord,
    ) -> Result<ListingAvailabilityMappingRecord, sqlx::Error> {
        sqlx::query("INSERT INTO listing_availability_mapping (raw, availability, mapping_type, created, updated) VALUES ($1, $2, $3, $4, $5) RETURNING raw, availability, mapping_type, decision_kind, created, updated")
            .bind(&record.raw).bind(record.availability.map(ListingAvailability::as_str)).bind(mapping_type_code(record.mapping_type)).bind(record.created).bind(record.updated).fetch_one(self.pool).await.and_then(row)
    }
    async fn update_mapping(
        &self,
        record: &ListingAvailabilityMappingRecord,
    ) -> Result<ListingAvailabilityMappingRecord, sqlx::Error> {
        sqlx::query("UPDATE listing_availability_mapping SET availability = $2, mapping_type = $3, updated = NOW() WHERE raw = $1 RETURNING raw, availability, mapping_type, decision_kind, created, updated")
            .bind(&record.raw).bind(record.availability.map(ListingAvailability::as_str)).bind(mapping_type_code(record.mapping_type)).fetch_one(self.pool).await.and_then(row)
    }
}
fn mapping_type_code(value: ListingAvailabilityMappingType) -> &'static str {
    match value {
        ListingAvailabilityMappingType::Value => "VALUE",
        ListingAvailabilityMappingType::Regex => "REGEX",
    }
}
