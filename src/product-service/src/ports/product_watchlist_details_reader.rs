use crate::ports::PersonalizedProductDetailsReadModel;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;
use common::user_id::UserId;
use localization::Language;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductWatchlistDetailsCursor {
    pub watchlist_created: OffsetDateTime,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductWatchlistDetailsRequest {
    pub user_id: UserId,
    pub language: Language,
    pub cursor: Cursor<ProductWatchlistDetailsCursor>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductWatchlistDetailsReadError {
    #[error("product watchlist details query failed")]
    QueryFailed,
    #[error("product watchlist details read model is invalid")]
    InvalidReadModel,
}

#[async_trait::async_trait]
pub trait ProductWatchlistDetailsReader: Send {
    async fn find_for_user(
        &mut self,
        request: &ProductWatchlistDetailsRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductDetailsReadModel, ProductWatchlistDetailsCursor>,
        ProductWatchlistDetailsReadError,
    >;
}

pub trait ProductWatchlistDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductWatchlistDetailsReader + 'tx;
}
