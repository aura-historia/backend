use product_listing_service::ports::{
    ProductListingEmbedding, ProductListingEmbeddingLookup, ProductListingEmbeddingReadError,
    ProductListingEmbeddingReader, ProductListingEmbeddingReaderFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEmbeddingReaderFactory;

struct SqlxProductListingEmbeddingReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingEmbeddingRow {
    product_listing_id: uuid::Uuid,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding query failed")]
struct ProductListingEmbeddingQuerySqlxError(#[source] sqlx::Error);

impl From<ProductListingEmbeddingRow> for ProductListingEmbedding {
    fn from(row: ProductListingEmbeddingRow) -> Self {
        Self {
            product_listing_id: row.product_listing_id.into(),
            embedding: row.embedding,
        }
    }
}

impl SqlxProductListingEmbeddingReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEmbeddingReaderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEmbeddingReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEmbeddingReader + 'tx {
        SqlxProductListingEmbeddingReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingReader for SqlxProductListingEmbeddingReader<'_> {
    async fn find_embedding(
        &mut self,
        lookup: &ProductListingEmbeddingLookup,
    ) -> Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError> {
        let query = match lookup {
            ProductListingEmbeddingLookup::ById(product_listing_id) => sqlx::query_as::<_, ProductListingEmbeddingRow>(
                "SELECT product_listing_id, embedding FROM product_listings WHERE product_listing_id = $1",
            )
            .bind(uuid::Uuid::from(*product_listing_id)),
            ProductListingEmbeddingLookup::BySlug { shop_slug_id, product_listing_slug_id } => sqlx::query_as::<_, ProductListingEmbeddingRow>(
                "SELECT p.product_listing_id, p.embedding FROM product_listings p JOIN shops s ON s.shop_id = p.shop_id WHERE s.shop_slug_id = $1 AND p.product_listing_slug_id = $2",
            )
            .bind(shop_slug_id.as_ref())
            .bind(product_listing_slug_id.as_ref()),
        };
        query
            .fetch_optional(&mut *self.connection)
            .await
            .map(|row| row.map(Into::into))
            .map_err(|source| {
                ProductListingEmbeddingReadError::ProductListingEmbeddingQueryFailed {
                    source: Box::new(ProductListingEmbeddingQuerySqlxError(source)),
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_embedding_row_with_missing_embedding() {
        let embedding: ProductListingEmbedding = ProductListingEmbeddingRow {
            product_listing_id: uuid::Uuid::nil(),
            embedding: None,
        }
        .into();

        assert_eq!(
            uuid::Uuid::nil(),
            uuid::Uuid::from(embedding.product_listing_id)
        );
        assert!(embedding.embedding.is_none());
    }
}
