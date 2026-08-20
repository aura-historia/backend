use common::error::boxed::BoxError;
use product_core::{product_id::ProductId, product_slug_id::ProductSlugId};
use shop_core::shop_slug_id::ShopSlugId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEmbedding {
    pub product_id: ProductId,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductEmbeddingLookup {
    ById(ProductId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductEmbeddingReadError {
    #[error("product embedding query failed")]
    ProductEmbeddingQueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductEmbeddingReader: Send {
    async fn find_embedding(
        &mut self,
        lookup: &ProductEmbeddingLookup,
    ) -> Result<Option<ProductEmbedding>, ProductEmbeddingReadError>;
}

pub trait ProductEmbeddingReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductEmbeddingReader + 'tx;
}
