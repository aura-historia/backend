use listing_source_postgres::{
    SqlxPublicListingSourceDetailsReaderFactory, SqlxPublicListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::{
        GetPublicListingSourceBySlugHandler, GetPublicListingSourceBySlugUseCase,
    },
    search_public_listing_sources::{
        SearchPublicListingSourcesHandler, SearchPublicListingSourcesUseCase,
    },
};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::sync::Arc;

/// Public discovery use cases shared by REST and MCP, with no transport state or DTOs.
#[derive(Clone)]
pub struct PublicListingSources {
    pub search: Arc<dyn SearchPublicListingSourcesUseCase>,
    pub get_by_slug: Arc<dyn GetPublicListingSourceBySlugUseCase>,
}

impl PublicListingSources {
    pub(super) fn compose(pool: &PgPool) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        Self {
            search: Arc::new(SearchPublicListingSourcesHandler::new(
                unit_of_work.clone(),
                SqlxPublicListingSourceSearchReaderFactory::new(),
            )),
            get_by_slug: Arc::new(GetPublicListingSourceBySlugHandler::new(
                unit_of_work,
                SqlxPublicListingSourceDetailsReaderFactory::new(),
            )),
        }
    }
}
