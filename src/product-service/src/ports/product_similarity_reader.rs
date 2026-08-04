use common::{
    error::boxed::BoxError,
    product_id::{ProductId, ProductKey},
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSimilaritySeed {
    pub product_id: ProductId,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSimilarityReadError {
    #[error("product similarity seed query failed")]
    ProductSimilarityQueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductSimilarityReader: Send {
    async fn find_seed(
        &mut self,
        product_key: &ProductKey,
    ) -> Result<Option<ProductSimilaritySeed>, ProductSimilarityReadError>;
}

pub trait ProductSimilarityReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductSimilarityReader + 'tx;
}
