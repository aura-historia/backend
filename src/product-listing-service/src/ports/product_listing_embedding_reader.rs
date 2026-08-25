use application::error::BoxError;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use shop_core::shop_slug_id::ShopSlugId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEmbedding {
    pub product_id: ProductListingId,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEmbeddingLookup {
    ById(ProductListingId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductListingSlugId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEmbeddingReadError {
    #[error("product embedding query failed")]
    ProductListingEmbeddingQueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingEmbeddingReader: Send {
    async fn find_embedding(
        &mut self,
        lookup: &ProductListingEmbeddingLookup,
    ) -> Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError>;
}

pub trait ProductListingEmbeddingReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEmbeddingReader + 'tx;
}
