use product_listing_service::ports::{
    ProductEmbedding, ProductEmbeddingLookup, ProductEmbeddingReadError, ProductEmbeddingReader,
    ProductEmbeddingReaderFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductEmbeddingReaderFactory;

struct SqlxProductEmbeddingReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductEmbeddingRow {
    product_id: uuid::Uuid,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding query failed")]
struct ProductEmbeddingQuerySqlxError(#[source] sqlx::Error);

impl From<ProductEmbeddingRow> for ProductEmbedding {
    fn from(row: ProductEmbeddingRow) -> Self {
        Self {
            product_id: row.product_id.into(),
            embedding: row.embedding,
        }
    }
}

impl SqlxProductEmbeddingReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductEmbeddingReaderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductEmbeddingReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductEmbeddingReader + 'tx {
        SqlxProductEmbeddingReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductEmbeddingReader for SqlxProductEmbeddingReader<'_> {
    async fn find_embedding(
        &mut self,
        lookup: &ProductEmbeddingLookup,
    ) -> Result<Option<ProductEmbedding>, ProductEmbeddingReadError> {
        let query = match lookup {
            ProductEmbeddingLookup::ById(product_id) => sqlx::query_as::<_, ProductEmbeddingRow>(
                "SELECT product_id, embedding FROM products WHERE product_id = $1",
            )
            .bind(uuid::Uuid::from(*product_id)),
            ProductEmbeddingLookup::BySlug { shop_slug_id, product_slug_id } => sqlx::query_as::<_, ProductEmbeddingRow>(
                "SELECT p.product_id, p.embedding FROM products p JOIN shops s ON s.shop_id = p.shop_id WHERE s.shop_slug_id = $1 AND p.product_slug_id = $2",
            )
            .bind(shop_slug_id.as_ref())
            .bind(product_slug_id.as_ref()),
        };
        query
            .fetch_optional(&mut *self.connection)
            .await
            .map(|row| row.map(Into::into))
            .map_err(
                |source| ProductEmbeddingReadError::ProductEmbeddingQueryFailed {
                    source: Box::new(ProductEmbeddingQuerySqlxError(source)),
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_embedding_row_with_missing_embedding() {
        let embedding: ProductEmbedding = ProductEmbeddingRow {
            product_id: uuid::Uuid::nil(),
            embedding: None,
        }
        .into();

        assert_eq!(uuid::Uuid::nil(), uuid::Uuid::from(embedding.product_id));
        assert!(embedding.embedding.is_none());
    }
}
