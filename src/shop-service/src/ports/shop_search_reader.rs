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
pub trait ShopSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchShopsRequest,
    ) -> Result<SearchShopsResult, ShopSearchReadError>;
}
