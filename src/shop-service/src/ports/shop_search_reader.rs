#![allow(dead_code)]

use crate::use_cases::queries::search_shops::{SearchShopsRequest, SearchShopsResult};

#[derive(Debug, thiserror::Error)]
pub enum ShopSearchReadError {
    #[error("temporary shop search failure")]
    TemporarilyUnavailable,
    #[error("invalid shop search read model")]
    InvalidReadModel,
    #[error("internal shop search failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait ShopSearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchShopsRequest,
    ) -> Result<SearchShopsResult, ShopSearchReadError>;
}

pub trait ShopSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShopSearchReader + 'tx;
}
