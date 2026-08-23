#![allow(dead_code)]

use crate::use_cases::queries::search_shops::{SearchShopsRequest, SearchShopsResult};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum ShopSearchReadError {
    #[error("temporary shop search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid shop search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal shop search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
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
