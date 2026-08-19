use common::{error::boxed::box_error, event_id::EventId, product_id::ProductId};
use platform_postgres::SqlxTransaction;
use product_service::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory,
};
use sqlx::PgConnection;

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
