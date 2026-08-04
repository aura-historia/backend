use common::product_id::ProductKey;
use product_service::ports::{
    ProductSimilarityReadError, ProductSimilarityReader, ProductSimilarityReaderFactory,
    ProductSimilaritySeed,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductSimilarityReaderFactory;

struct SqlxProductSimilarityReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductSimilaritySeedRow {
    product_id: uuid::Uuid,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
#[error("product similarity seed query failed")]
struct ProductSimilaritySeedQuerySqlxError(#[source] sqlx::Error);

impl From<ProductSimilaritySeedRow> for ProductSimilaritySeed {
    fn from(row: ProductSimilaritySeedRow) -> Self {
        Self {
            product_id: row.product_id.into(),
            embedding: row.embedding,
        }
    }
}

impl SqlxProductSimilarityReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductSimilarityReaderFactory<common::postgres::SqlxTransaction>
    for SqlxProductSimilarityReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut common::postgres::SqlxTransaction,
    ) -> impl ProductSimilarityReader + 'tx {
        SqlxProductSimilarityReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductSimilarityReader for SqlxProductSimilarityReader<'_> {
    async fn find_seed(
        &mut self,
        product_key: &ProductKey,
    ) -> Result<Option<ProductSimilaritySeed>, ProductSimilarityReadError> {
        sqlx::query_as::<_, ProductSimilaritySeedRow>(
            r#"
            SELECT product_id, embedding
            FROM products
            WHERE shop_id = $1 AND shops_product_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(product_key.shop_id))
        .bind(product_key.shops_product_id.as_ref())
        .fetch_optional(&mut *self.connection)
        .await
        .map(|row| row.map(Into::into))
        .map_err(
            |source| ProductSimilarityReadError::ProductSimilarityQueryFailed {
                source: Box::new(ProductSimilaritySeedQuerySqlxError(source)),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_seed_row_with_missing_embedding() {
        let seed: ProductSimilaritySeed = ProductSimilaritySeedRow {
            product_id: uuid::Uuid::nil(),
            embedding: None,
        }
        .into();

        assert_eq!(uuid::Uuid::nil(), uuid::Uuid::from(seed.product_id));
        assert!(seed.embedding.is_none());
    }
}
