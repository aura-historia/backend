mod listing_source_details_reader;
mod listing_source_search_reader;
mod public_listing_source;
mod public_listing_source_details_reader;
mod public_listing_source_search_reader;
mod shopify_source_reader;
mod web_crawl_source_reader;
mod woocommerce_signature_verifier;
mod woocommerce_source_reader;

pub use listing_source_search_reader::SqlxListingSourceSearchReaderFactory;
pub use public_listing_source_details_reader::SqlxPublicListingSourceDetailsReaderFactory;
pub use public_listing_source_search_reader::SqlxPublicListingSourceSearchReaderFactory;

use application::error::box_error;
use listing_source_service::ports::ListingSourceReadError;
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxListingSourceReaders {
    pub(super) pool: PgPool,
}

impl SqlxListingSourceReaders {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub(super) fn read_error(error: sqlx::Error) -> ListingSourceReadError {
    ListingSourceReadError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

pub(super) fn invalid_read(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ListingSourceReadError {
    ListingSourceReadError::InvalidReadModel {
        source: box_error(error),
    }
}
