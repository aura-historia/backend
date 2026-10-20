use embedding::EmbeddingGenerator;
use fxrate_postgres::{SqlxFxRateSnapshotReader, SqlxFxRateSnapshotRepositoryFactory};
use fxrate_service::readers::{CachedFxRateSnapshotReader, FxSearchCacheConfig};
use opensearch::OpenSearch;
use platform_postgres::SqlxUnitOfWork;
use product_listing_opensearch::{
    OpenSearchProductListingSearchReader, OpenSearchProductListingSimilarProductListingsReader,
};
use product_listing_postgres::{
    SqlxListingSourceSummaryReader, SqlxProductListingContentAssessmentReader,
    SqlxProductListingDetailsReaderFactory, SqlxProductListingEmbeddingReaderFactory,
    SqlxProductListingUserStateReader,
};
use product_listing_service::{
    readers::{CachedListingSourceSummaryReader, SourceSearchCacheConfig},
    shipping::ShippingEnrichment,
    use_cases::{
        GetProductListingHandler, GetProductListingUseCase, GetSimilarProductListingsHandler,
        GetSimilarProductListingsUseCase, ProductListingSearchReadExecutionPolicy,
        SearchProductListingsHandler, SearchProductListingsUseCase,
    },
};
use sqlx::PgPool;
use std::sync::Arc;

/// Only provider handles and policies required by listing discovery. Selecting unrelated
/// runtime families never loads OpenSearch or embedding-provider configuration.
pub struct ProductListingDependencies {
    pub opensearch: OpenSearch,
    pub embeddings: Arc<dyn EmbeddingGenerator>,
    pub fx_cache: FxSearchCacheConfig,
    pub source_cache: SourceSearchCacheConfig,
    pub read_execution_policy: ProductListingSearchReadExecutionPolicy,
}
/// Listing inbound use cases shared by REST and MCP, including their existing personalization.
#[derive(Clone)]
pub struct PublicProductListings {
    pub get: Arc<dyn GetProductListingUseCase>,
    pub similar: Arc<dyn GetSimilarProductListingsUseCase>,
    pub search: Arc<dyn SearchProductListingsUseCase>,
}
impl PublicProductListings {
    pub(super) fn compose(
        pool: &PgPool,
        shipping: ShippingEnrichment,
        dependencies: ProductListingDependencies,
    ) -> Self {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let user_states = SqlxProductListingUserStateReader::new(pool.clone());
        Self {
            get: Arc::new(GetProductListingHandler::new(
                unit_of_work.clone(),
                SqlxProductListingDetailsReaderFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                shipping.clone(),
            )),
            similar: Arc::new(GetSimilarProductListingsHandler::new(
                unit_of_work,
                SqlxProductListingEmbeddingReaderFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                OpenSearchProductListingSimilarProductListingsReader::new(
                    dependencies.opensearch.clone(),
                ),
                SqlxListingSourceSummaryReader::new(pool.clone()),
                user_states.clone(),
                SqlxProductListingContentAssessmentReader::new(pool.clone()),
                shipping.clone(),
            )),
            search: Arc::new(
                SearchProductListingsHandler::new(
                    OpenSearchProductListingSearchReader::new(dependencies.opensearch),
                    CachedFxRateSnapshotReader::new(
                        SqlxFxRateSnapshotReader::new(pool.clone()),
                        dependencies.fx_cache,
                    ),
                    dependencies.embeddings,
                    CachedListingSourceSummaryReader::new(
                        SqlxListingSourceSummaryReader::new(pool.clone()),
                        dependencies.source_cache,
                    ),
                    user_states,
                    SqlxProductListingContentAssessmentReader::new(pool.clone()),
                    shipping,
                )
                .with_read_execution_policy(dependencies.read_execution_policy),
            ),
        }
    }
}
/// Shared composition of existing shipping assessment and address resolution services.
pub(super) fn shipping(pool: &PgPool) -> ShippingEnrichment {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let address_book = Arc::new(user_service::address_book::AddressBookHandler::new(
        unit_of_work.clone(),
        user_postgres::SqlxAddressBookFactory,
        geo_postgres::SqlxResolutionRepositoryFactory,
    ));
    ShippingEnrichment::new(
        Arc::new(shipping_service::assessment::AssessDeliveryHandler::new(
            unit_of_work,
            shipping_postgres::assessment::SqlxAssessmentReaderFactory,
            address_book,
        )),
        Arc::new(SqlxFxRateSnapshotReader::new(pool.clone())),
    )
}
