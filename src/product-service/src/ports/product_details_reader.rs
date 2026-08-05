#![allow(dead_code)]

use crate::use_cases::queries::get_product::{PersonalizedProductDetailsView, ProductLookup};
use common::language::domain::Language;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsReadRequest {
    pub lookup: ProductLookup,
    pub language: Language,
    pub user_id: Option<UserId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsReadError {
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductDetailsReader: Send {
    async fn find_details(
        &mut self,
        request: &ProductDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductDetailsView>, ProductDetailsReadError>;
}

pub trait ProductDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductDetailsReader + 'tx;
}
