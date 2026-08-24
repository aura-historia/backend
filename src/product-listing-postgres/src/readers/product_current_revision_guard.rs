use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_id::ProductId;
use product_listing_service::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory, ProductCurrentRevisionRef,
};
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductCurrentRevisionGuardFactory;

struct SqlxProductCurrentRevisionGuard<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product current revision guard SQL query failed")]
struct ProductCurrentRevisionGuardSqlxError(#[source] sqlx::Error);

impl SqlxProductCurrentRevisionGuardFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductCurrentRevisionGuardFactory<SqlxTransaction>
    for SqlxProductCurrentRevisionGuardFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductCurrentRevisionGuard + 'tx {
        SqlxProductCurrentRevisionGuard {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductCurrentRevisionGuard for SqlxProductCurrentRevisionGuard<'_> {
    async fn lock_and_check(
        &mut self,
        product_id: ProductId,
        expected_event_id: EventId,
    ) -> Result<ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError> {
        let event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1 FOR SHARE",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| ProductCurrentRevisionCheckError::CheckFailed {
            source: box_error(ProductCurrentRevisionGuardSqlxError(source)),
        })?;

        Ok(match event_id {
            Some(event_id) if EventId::from(event_id) == expected_event_id => {
                ProductCurrentRevisionCheck::Current
            }
            Some(_) | None => ProductCurrentRevisionCheck::Stale,
        })
    }

    async fn lock_and_check_all(
        &mut self,
        refs: &[ProductCurrentRevisionRef],
    ) -> Result<
        HashMap<ProductCurrentRevisionRef, ProductCurrentRevisionCheck>,
        ProductCurrentRevisionCheckError,
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
        .map_err(|source| ProductCurrentRevisionCheckError::CheckFailed {
            source: box_error(ProductCurrentRevisionGuardSqlxError(source)),
        })?
        .into_iter()
        .collect::<HashMap<_, _>>();

        Ok(refs
            .iter()
            .copied()
            .map(|reference| {
                let check = match current_event_ids.get(&uuid::Uuid::from(reference.product_id)) {
                    Some(event_id) if EventId::from(*event_id) == reference.expected_event_id => {
                        ProductCurrentRevisionCheck::Current
                    }
                    Some(_) | None => ProductCurrentRevisionCheck::Stale,
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
        let error = ProductCurrentRevisionCheckError::CheckFailed {
            source: box_error(ProductCurrentRevisionGuardSqlxError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductCurrentRevisionCheckError::CheckFailed { source } = error;
        assert!(
            source
                .downcast_ref::<ProductCurrentRevisionGuardSqlxError>()
                .is_some()
        );
    }
}
