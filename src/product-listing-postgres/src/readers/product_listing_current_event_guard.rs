use crate::object_id::try_from_uuid;
use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingCurrentEventCheckError, ProductListingCurrentEventGuard,
    ProductListingCurrentEventGuardFactory,
};
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingCurrentEventGuardFactory;

struct SqlxProductListingCurrentEventGuard<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product current event guard SQL query failed")]
struct ProductListingCurrentEventGuardSqlxError(#[source] sqlx::Error);

impl SqlxProductListingCurrentEventGuardFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingCurrentEventGuardFactory<SqlxTransaction>
    for SqlxProductListingCurrentEventGuardFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingCurrentEventGuard + 'tx {
        SqlxProductListingCurrentEventGuard {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingCurrentEventGuard for SqlxProductListingCurrentEventGuard<'_> {
    async fn lock_current_events(
        &mut self,
        product_listing_ids: &[ProductListingId],
    ) -> Result<HashMap<ProductListingId, EventId>, ProductListingCurrentEventCheckError> {
        if product_listing_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let product_listing_ids = product_listing_ids
            .iter()
            .map(|product_listing_id| product_listing_id.into_uuid())
            .collect::<Vec<_>>();
        sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
            r#"
            SELECT product_listing_id, current_event_id
            FROM product_listings
            WHERE product_listing_id = ANY($1::uuid[])
            FOR SHARE
            "#,
        )
        .bind(product_listing_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|source| ProductListingCurrentEventCheckError::CheckFailed {
            source: box_error(ProductListingCurrentEventGuardSqlxError(source)),
        })?
        .into_iter()
        .map(|(product_listing_id, current_event_id)| {
            Ok((
                try_from_uuid::<ProductListingId>(product_listing_id, "ProductListing ID")
                    .map_err(invalid_row)?,
                try_from_uuid::<EventId>(current_event_id, "current event ID")
                    .map_err(invalid_row)?,
            ))
        })
        .collect()
    }
}

fn invalid_row(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingCurrentEventCheckError {
    ProductListingCurrentEventCheckError::CheckFailed {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error = ProductListingCurrentEventCheckError::CheckFailed {
            source: box_error(ProductListingCurrentEventGuardSqlxError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductListingCurrentEventCheckError::CheckFailed { source } = error;
        assert!(
            source
                .downcast_ref::<ProductListingCurrentEventGuardSqlxError>()
                .is_some()
        );
    }
}
