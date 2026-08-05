use common::product_id::{ProductId, ProductKey};

#[derive(Debug, thiserror::Error)]
pub enum ProductIdentityReadError {
    #[error("product identity lookup failed")]
    LookupFailed,
}

#[async_trait::async_trait]
pub trait ProductIdentityReader: Send + Sync {
    async fn find_id_by_key(
        &self,
        product_key: &ProductKey,
    ) -> Result<Option<ProductId>, ProductIdentityReadError>;
}
