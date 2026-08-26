use crate::ports::PersonalizedProductListingDetailsReadModel;
use application::pagination::{Cursor, CursoredResult};
use localization::Language;
use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingWatchlistDetailsCursor {
    pub watchlist_created: OffsetDateTime,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingWatchlistDetailsRequest {
    pub user_id: UserId,
    pub language: Language,
    pub cursor: Cursor<ProductListingWatchlistDetailsCursor>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingWatchlistDetailsReadError {
    #[error("product watchlist details query failed")]
    QueryFailed,
    #[error("product watchlist details read model is invalid")]
    InvalidReadModel,
}

#[async_trait::async_trait]
pub trait ProductListingWatchlistDetailsReader: Send {
    async fn find_for_user(
        &mut self,
        request: &ProductListingWatchlistDetailsRequest,
    ) -> Result<
        CursoredResult<
            PersonalizedProductListingDetailsReadModel,
            ProductListingWatchlistDetailsCursor,
        >,
        ProductListingWatchlistDetailsReadError,
    >;
}

pub trait ProductListingWatchlistDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingWatchlistDetailsReader + 'tx;
}
