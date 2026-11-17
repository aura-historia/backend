use crate::object_id::try_from_uuid;
use application::error::box_error;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingProjectionTrigger, ProductListingProjectionTriggerReadError,
    ProductListingProjectionTriggerReader,
};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct SqlxProductListingProjectionTriggerReader {
    pool: PgPool,
}

impl SqlxProductListingProjectionTriggerReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ProjectionTriggerRow {
    product_listing_id: uuid::Uuid,
    current_event_id: uuid::Uuid,
}

#[derive(Debug, thiserror::Error)]
#[error("product listing projection trigger SQL query failed")]
struct ProjectionTriggerQueryError(#[source] sqlx::Error);

impl TryFrom<ProjectionTriggerRow> for ProductListingProjectionTrigger {
    type Error = ProductListingProjectionTriggerReadError;

    fn try_from(row: ProjectionTriggerRow) -> Result<Self, Self::Error> {
        let invalid = |source| ProductListingProjectionTriggerReadError::InvalidPersistedState {
            source: box_error(source),
        };
        Ok(Self {
            product_listing_id: try_from_uuid::<ProductListingId>(
                row.product_listing_id,
                "ProductListing ID",
            )
            .map_err(invalid)?,
            current_event_id: try_from_uuid::<EventId>(row.current_event_id, "event ID")
                .map_err(invalid)?,
        })
    }
}

#[async_trait::async_trait]
impl ProductListingProjectionTriggerReader for SqlxProductListingProjectionTriggerReader {
    async fn page_after(
        &self,
        after: Option<ProductListingId>,
        limit: u32,
    ) -> Result<Vec<ProductListingProjectionTrigger>, ProductListingProjectionTriggerReadError>
    {
        let rows = sqlx::query_as::<_, ProjectionTriggerRow>(
            "SELECT product_listing_id, current_event_id \
             FROM product_listings \
             WHERE $1::uuid IS NULL OR product_listing_id > $1 \
             ORDER BY product_listing_id \
             LIMIT $2",
        )
        .bind(after.map(ProductListingId::into_uuid))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(
            |source| ProductListingProjectionTriggerReadError::QueryFailed {
                source: box_error(ProjectionTriggerQueryError(source)),
            },
        )?;
        rows.into_iter().map(TryFrom::try_from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_a_row_to_its_trigger() {
        let product_listing_id = ProductListingId::new();
        let current_event_id = EventId::new();

        let trigger = ProductListingProjectionTrigger::try_from(ProjectionTriggerRow {
            product_listing_id: product_listing_id.into_uuid(),
            current_event_id: current_event_id.into_uuid(),
        })
        .unwrap();

        assert_eq!(
            ProductListingProjectionTrigger {
                product_listing_id,
                current_event_id
            },
            trigger
        );
    }

    #[test]
    fn should_fail_closed_on_a_persisted_id_that_is_not_an_object_id() {
        let result = ProductListingProjectionTrigger::try_from(ProjectionTriggerRow {
            product_listing_id: uuid::Uuid::new_v4(),
            current_event_id: EventId::new().into_uuid(),
        });

        assert!(matches!(
            result,
            Err(ProductListingProjectionTriggerReadError::InvalidPersistedState { .. })
        ));
    }
}
