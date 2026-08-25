use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingCurrentRevisionCheck, ProductListingCurrentRevisionCheckError,
    ProductListingCurrentRevisionGuard, ProductListingCurrentRevisionGuardFactory,
    ProductListingCurrentRevisionRef,
};
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingCurrentRevisionGuardFactory;

struct SqlxProductListingCurrentRevisionGuard<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product current revision guard SQL query failed")]
struct ProductListingCurrentRevisionGuardSqlxError(#[source] sqlx::Error);

impl SqlxProductListingCurrentRevisionGuardFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingCurrentRevisionGuardFactory<SqlxTransaction>
    for SqlxProductListingCurrentRevisionGuardFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingCurrentRevisionGuard + 'tx {
        SqlxProductListingCurrentRevisionGuard {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingCurrentRevisionGuard for SqlxProductListingCurrentRevisionGuard<'_> {
    async fn lock_and_check(
        &mut self,
        product_id: ProductListingId,
        expected_event_id: EventId,
    ) -> Result<ProductListingCurrentRevisionCheck, ProductListingCurrentRevisionCheckError> {
        let event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1 FOR SHARE",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(
            |source| ProductListingCurrentRevisionCheckError::CheckFailed {
                source: box_error(ProductListingCurrentRevisionGuardSqlxError(source)),
            },
        )?;

        Ok(match event_id {
            Some(event_id) if EventId::from(event_id) == expected_event_id => {
                ProductListingCurrentRevisionCheck::Current
            }
            Some(_) | None => ProductListingCurrentRevisionCheck::Stale,
        })
    }

    async fn lock_and_check_all(
        &mut self,
        refs: &[ProductListingCurrentRevisionRef],
    ) -> Result<
        HashMap<ProductListingCurrentRevisionRef, ProductListingCurrentRevisionCheck>,
        ProductListingCurrentRevisionCheckError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }

        let product_ids = refs
            .iter()
            .map(|reference| uuid::Uuid::from(reference.product_id))
            .collect::<Vec<_>>();
        let current_event_ids = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
            r#"
            SELECT product_id, event_id
            FROM products
            WHERE product_id = ANY($1::uuid[])
            FOR SHARE
            "#,
        )
        .bind(product_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(
            |source| ProductListingCurrentRevisionCheckError::CheckFailed {
                source: box_error(ProductListingCurrentRevisionGuardSqlxError(source)),
            },
        )?
        .into_iter()
        .collect::<HashMap<_, _>>();

        Ok(refs
            .iter()
            .copied()
            .map(|reference| {
                let check = match current_event_ids.get(&uuid::Uuid::from(reference.product_id)) {
                    Some(event_id) if EventId::from(*event_id) == reference.expected_event_id => {
                        ProductListingCurrentRevisionCheck::Current
                    }
                    Some(_) | None => ProductListingCurrentRevisionCheck::Stale,
                };
                (reference, check)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error = ProductListingCurrentRevisionCheckError::CheckFailed {
            source: box_error(ProductListingCurrentRevisionGuardSqlxError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductListingCurrentRevisionCheckError::CheckFailed { source } = error;
        assert!(
            source
                .downcast_ref::<ProductListingCurrentRevisionGuardSqlxError>()
                .is_some()
        );
    }
}
