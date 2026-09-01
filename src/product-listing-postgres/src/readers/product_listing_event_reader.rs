use application::error::box_error;
use domain_primitives::event_id::EventId;
use product_listing_service::{
    ports::{
        ProductListingEventReadError, ProductListingEventReader, ProductListingEventReaderFactory,
    },
    use_cases::{ProductListingEvent, ProductListingEventLookup},
};
use serde_json::Value;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::product_listing_event_codec;

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
    event_type_schema_version: i16,
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
            ProductListingEventLookup::ByTitleSlug(product_listing_title_slug_id) => {
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT product_listing_id FROM product_listings WHERE product_listing_title_slug_id = $1",
                )
                .bind(product_listing_title_slug_id.as_ref())
                .fetch_optional(&mut *self.connection)
                .await
            }
        }
        .map_err(|error| ProductListingEventReadError::ProductListingEventQueryFailed {
            source: box_error(ProductListingEventQuerySqlxError(error)),
        })?;

        let Some(product_listing_id) = product_listing_id else {
            return Ok(None);
        };

        let rows = sqlx::query_as::<_, ProductListingEventRow>(
            r#"
            SELECT
                event_id,
                product_listing_id,
                event_type,
                event_type_schema_version,
                payload,
                event_time
            FROM product_listing_events
            WHERE product_listing_id = $1
              AND event_group = 'DOMAIN'
              AND event_type IN (
                  'PRODUCT_LISTING_DISCOVERED',
                  'PRODUCT_LISTING_CHANGED'
              )
            ORDER BY event_time ASC, event_id ASC
            "#,
        )
        .bind(product_listing_id)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(
            |error| ProductListingEventReadError::ProductListingEventQueryFailed {
                source: box_error(ProductListingEventQuerySqlxError(error)),
            },
        )?;

        rows.into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("product listing event SQL query failed")]
struct ProductListingEventQuerySqlxError(#[source] sqlx::Error);

impl TryFrom<ProductListingEventRow> for ProductListingEvent {
    type Error = ProductListingEventReadError;

    fn try_from(row: ProductListingEventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_listing_id: row.product_listing_id.into(),
            event_id: EventId::from(row.event_id),
            payload: product_listing_event_codec::decode(
                &row.event_type,
                row.event_type_schema_version,
                &row.payload,
            )
            .map_err(|error| {
                ProductListingEventReadError::ProductListingEventReadModelInvalid {
                    source: box_error(error),
                }
            })?,
            timestamp: row.event_time,
        })
    }
}
