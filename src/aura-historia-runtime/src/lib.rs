//! Shared, transport-independent construction of selected application service families.
//!
//! This composition root owns concrete pools and adapter wiring. Callers select explicit
//! typed families; transport state, DTO mapping, authentication and business behavior stay
//! with their existing owners. Extract further families only when a transport needs them.

mod access_token_authentication;
mod notifications;
mod partner_listing_management;
mod partner_product_listing_batches;
mod partner_product_listing_ingestion;
mod party_locations;
mod postgres;
mod private_addresses;
pub mod product_listing_config;
mod product_listing_delivery;
mod product_listing_providers;
mod public_auctions;
mod public_listing_source_read_budget;
mod public_listing_sources;
mod public_overview;
mod public_party_locations;
mod public_product_listing_history;
mod public_product_listings;
mod saved_search_previews;
mod saved_searches;
mod search_intent_translation;
mod shipping_management;
mod source_location_defaults;
mod user_profile;
mod watchlist;

pub use notifications::Notifications;
pub use partner_listing_management::PartnerListingManagement;
pub use partner_product_listing_batches::PartnerProductListingBatches;
pub use partner_product_listing_ingestion::{
    PartnerProductListingIngestion, ProductListingIngestionDependencies, with_publication_deadline,
};
pub use party_locations::PartyLocations;
pub use postgres::PostgresConfigError;
pub use private_addresses::PrivateAddresses;
pub use product_listing_config::ProductListingConfig;
pub use product_listing_delivery::ProductListingDelivery;
pub use product_listing_providers::{
    PRODUCT_LISTING_OPENSEARCH_TIMEOUT, ProductListingProviderError,
};
pub use public_auctions::PublicAuctions;
pub use public_listing_source_read_budget::PublicListingSourceReadBudget;
pub use public_listing_sources::PublicListingSources;
pub use public_overview::PublicOverview;
pub use public_party_locations::PublicPartyLocations;
pub use public_product_listing_history::PublicProductListingHistory;
pub use public_product_listings::{ProductListingDependencies, PublicProductListings};
pub use saved_search_previews::{
    CLASSIFIER_MODEL_ENV, CLASSIFIER_MODEL_PROVIDER_ENV, CLOUDFLARE_ACCOUNT_ID_ENV,
    CLOUDFLARE_API_TOKEN_ENV, SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS_ENV,
    SavedSearchPreviewConfig, SavedSearchPreviewConfigError, SavedSearchPreviewDependencies,
    SavedSearchPreviews,
};
pub use saved_searches::SavedSearches;
pub use search_intent_translation::{
    SearchIntentTranslatorConfigError, UnavailableSearchIntentTranslator, search_intent_translator,
    search_intent_translator_from_env,
};
pub use shipping_management::ShippingManagement;
pub use source_location_defaults::SourceLocationDefaults;
pub use user_profile::UserProfile;
pub use watchlist::Watchlist;

use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use user_service::use_cases::AuthenticateAccessTokenUseCase;

/// One warm composition. Selecting a family constructs its handlers once for this pool.
pub struct Runtime {
    postgres: PgPool,
    product_listing_delivery: OnceLock<ProductListingDelivery>,
    notifications: OnceLock<Notifications>,
    partner_listing_management: OnceLock<PartnerListingManagement>,
    partner_product_listing_ingestion: OnceLock<PartnerProductListingIngestion>,
    partner_product_listing_batches: OnceLock<PartnerProductListingBatches>,
    party_locations: OnceLock<PartyLocations>,
    source_location_defaults: OnceLock<SourceLocationDefaults>,
    user_profile: OnceLock<UserProfile>,
    private_addresses: OnceLock<PrivateAddresses>,
    access_token_authentication: OnceLock<Arc<dyn AuthenticateAccessTokenUseCase>>,
    public_listing_sources: OnceLock<PublicListingSources>,
    public_product_listing_history: OnceLock<PublicProductListingHistory>,
    public_auctions: OnceLock<PublicAuctions>,
    public_product_listings: OnceLock<PublicProductListings>,
    listing_shipping: OnceLock<product_listing_service::shipping::ShippingEnrichment>,
    public_party_locations: OnceLock<PublicPartyLocations>,
    public_overview: OnceLock<PublicOverview>,
    saved_searches: OnceLock<SavedSearches>,
    saved_search_previews: OnceLock<SavedSearchPreviews>,
    watchlist: OnceLock<Watchlist>,
    shipping_management: OnceLock<ShippingManagement>,
}

impl Runtime {
    pub fn from_pool(postgres: PgPool) -> Self {
        Self {
            postgres,
            product_listing_delivery: OnceLock::new(),
            notifications: OnceLock::new(),
            partner_listing_management: OnceLock::new(),
            partner_product_listing_ingestion: OnceLock::new(),
            partner_product_listing_batches: OnceLock::new(),
            party_locations: OnceLock::new(),
            source_location_defaults: OnceLock::new(),
            user_profile: OnceLock::new(),
            private_addresses: OnceLock::new(),
            access_token_authentication: OnceLock::new(),
            public_listing_sources: OnceLock::new(),
            public_product_listing_history: OnceLock::new(),
            public_auctions: OnceLock::new(),
            public_product_listings: OnceLock::new(),
            listing_shipping: OnceLock::new(),
            public_party_locations: OnceLock::new(),
            public_overview: OnceLock::new(),
            saved_searches: OnceLock::new(),
            saved_search_previews: OnceLock::new(),
            watchlist: OnceLock::new(),
            shipping_management: OnceLock::new(),
        }
    }

    /// Startup with the explicit PostgreSQL environment configuration. Lambda functions receive
    /// these credentials at deployment, so one warm composition serves every invocation.
    pub async fn from_env() -> Result<Self, RuntimeError> {
        let config = postgres::pool_config_from_env()?;
        let pool = config.connect().await.map_err(RuntimeError::Pool)?;
        Ok(Self::from_pool(pool))
    }

    /// Shared pool for composition of families that have not been extracted yet.
    /// Request handlers must use inbound use cases rather than this pool.
    pub fn postgres_pool(&self) -> &PgPool {
        &self.postgres
    }

    pub fn user_profile(&self) -> &UserProfile {
        self.user_profile
            .get_or_init(|| UserProfile::compose(&self.postgres))
    }

    pub fn private_addresses(&self) -> &PrivateAddresses {
        self.private_addresses
            .get_or_init(|| PrivateAddresses::compose(&self.postgres))
    }

    pub fn access_token_authentication(&self) -> &Arc<dyn AuthenticateAccessTokenUseCase> {
        self.access_token_authentication
            .get_or_init(|| access_token_authentication::compose(&self.postgres))
    }

    pub fn public_product_listings(
        &self,
        dependencies: ProductListingDependencies,
    ) -> &PublicProductListings {
        self.public_product_listings.get_or_init(|| {
            PublicProductListings::compose(
                &self.postgres,
                self.listing_shipping().clone(),
                dependencies,
            )
        })
    }

    pub fn listing_shipping(&self) -> &product_listing_service::shipping::ShippingEnrichment {
        self.listing_shipping
            .get_or_init(|| public_product_listings::shipping(&self.postgres))
    }

    pub fn public_product_listing_history(&self) -> &PublicProductListingHistory {
        self.public_product_listing_history
            .get_or_init(|| PublicProductListingHistory::compose(&self.postgres))
    }

    pub fn public_auctions(&self) -> &PublicAuctions {
        self.public_auctions.get_or_init(|| {
            PublicAuctions::compose(&self.postgres, self.listing_shipping().clone())
        })
    }

    pub fn public_listing_sources(&self) -> &PublicListingSources {
        self.public_listing_sources
            .get_or_init(|| PublicListingSources::compose(&self.postgres))
    }

    pub fn public_party_locations(&self) -> &PublicPartyLocations {
        self.public_party_locations
            .get_or_init(|| PublicPartyLocations::compose(&self.postgres))
    }

    pub fn saved_searches(
        &self,
        embeddings: Arc<dyn embedding::EmbeddingGenerator>,
        translator: Arc<dyn search_filter_service::ports::SearchIntentTranslator>,
    ) -> &SavedSearches {
        self.saved_searches.get_or_init(|| {
            SavedSearches::compose(
                &self.postgres,
                self.listing_shipping().clone(),
                embeddings,
                translator,
            )
        })
    }

    /// REST-only: previews classify candidates, so MCP does not compose them.
    pub fn saved_search_previews(
        &self,
        listing: &ProductListingDependencies,
        dependencies: SavedSearchPreviewDependencies,
    ) -> &SavedSearchPreviews {
        self.saved_search_previews.get_or_init(|| {
            SavedSearchPreviews::compose(
                &self.postgres,
                self.listing_shipping().clone(),
                listing,
                dependencies,
            )
        })
    }

    pub fn watchlist(&self) -> &Watchlist {
        self.watchlist
            .get_or_init(|| Watchlist::compose(&self.postgres, self.listing_shipping().clone()))
    }

    pub fn product_listing_delivery(&self) -> &ProductListingDelivery {
        self.product_listing_delivery
            .get_or_init(|| ProductListingDelivery::compose(&self.postgres))
    }

    pub fn partner_product_listing_ingestion(
        &self,
        dependencies: ProductListingIngestionDependencies,
    ) -> &PartnerProductListingIngestion {
        self.partner_product_listing_ingestion
            .get_or_init(|| PartnerProductListingIngestion::compose(&self.postgres, dependencies))
    }

    pub fn partner_product_listing_batches(&self) -> &PartnerProductListingBatches {
        self.partner_product_listing_batches
            .get_or_init(|| PartnerProductListingBatches::compose(&self.postgres))
    }

    pub fn partner_listing_management(&self) -> &PartnerListingManagement {
        self.partner_listing_management
            .get_or_init(|| PartnerListingManagement::compose(&self.postgres))
    }

    pub fn shipping_management(&self) -> &ShippingManagement {
        self.shipping_management
            .get_or_init(|| ShippingManagement::compose(&self.postgres))
    }

    pub fn notifications(&self) -> &Notifications {
        self.notifications
            .get_or_init(|| Notifications::compose(&self.postgres))
    }

    pub fn party_locations(&self) -> &PartyLocations {
        self.party_locations
            .get_or_init(|| PartyLocations::compose(&self.postgres))
    }

    pub fn source_location_defaults(&self) -> &SourceLocationDefaults {
        self.source_location_defaults
            .get_or_init(|| SourceLocationDefaults::compose(&self.postgres))
    }

    pub fn public_overview(&self) -> &PublicOverview {
        self.public_overview
            .get_or_init(|| PublicOverview::compose(&self.postgres))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("invalid PostgreSQL runtime configuration")]
    PostgresConfig(#[from] PostgresConfigError),
    #[error("failed to create PostgreSQL runtime pool")]
    Pool(#[source] sqlx::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use listing_source_core::ListingSourceSlugId;
    use listing_source_service::use_cases::queries::{
        get_public_listing_source_by_slug::{
            GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
        },
        search_public_listing_sources::{
            PublicListingSourceSearchQuery, SearchPublicListingSourcesError,
            SearchPublicListingSourcesRequest,
        },
    };
    use std::{error::Error, process::Command, sync::Arc};

    type TestError = Box<dyn Error + Send + Sync>;

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("runtime-test"),
            correlation_id: CorrelationId::new("runtime-test"),
        }
    }

    fn search(query: Option<&str>) -> Result<SearchPublicListingSourcesRequest, TestError> {
        Ok(SearchPublicListingSourcesRequest::new(
            PublicListingSourceSearchQuery::new(query.map(str::to_owned))?,
            21,
            None,
        )?)
    }

    fn run_probe(name: &str) -> Result<(), TestError> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--exact", name, "--ignored", "--nocapture"])
            .env_clear()
            .envs([
                ("POSTGRES_HOST", "localhost"),
                ("POSTGRES_PORT", "9"),
                ("POSTGRES_DATABASE", "fixture"),
                ("POSTGRES_USERNAME", "fixture"),
                ("POSTGRES_PASSWORD", "fixture-password"),
                ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem"),
            ]);
        // Keep subprocess coverage in CI's per-process output directory.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
        Ok(())
    }

    #[test]
    fn should_start_anonymous_runtime_without_unrelated_provider_configuration()
    -> Result<(), TestError> {
        run_probe("tests::anonymous_bootstrap_probe")
    }

    #[tokio::test]
    #[ignore = "invoked by the anonymous startup test in a clean subprocess"]
    async fn anonymous_bootstrap_probe() -> Result<(), TestError> {
        let runtime = Runtime::from_env().await?;
        let profile = runtime.user_profile();
        assert!(Arc::ptr_eq(&profile.get, &runtime.user_profile().get));
        assert!(Arc::ptr_eq(&profile.update, &runtime.user_profile().update));
        let addresses = runtime.private_addresses();
        assert!(Arc::ptr_eq(
            &addresses.commands,
            &runtime.private_addresses().commands
        ));
        assert!(Arc::ptr_eq(
            &addresses.queries,
            &runtime.private_addresses().queries
        ));
        let family = runtime.public_listing_sources();
        let same = runtime.public_listing_sources();
        assert!(Arc::ptr_eq(&family.search, &same.search));
        assert!(Arc::ptr_eq(&family.get_by_slug, &same.get_by_slug));
        assert!(Arc::ptr_eq(
            &runtime.public_party_locations().discover,
            &runtime.public_party_locations().discover,
        ));
        assert!(Arc::ptr_eq(
            &runtime.public_overview().get,
            &runtime.public_overview().get,
        ));
        let auctions = runtime.public_auctions();
        let same_auctions = runtime.public_auctions();
        assert!(Arc::ptr_eq(&auctions.get, &same_auctions.get));
        assert!(Arc::ptr_eq(&auctions.list, &same_auctions.list));
        assert!(Arc::ptr_eq(&auctions.catalogue, &same_auctions.catalogue));
        let result = family
            .search
            .execute(&context(), search(Some("a"))?)
            .await?;
        assert!(result.items.is_empty());
        assert!(result.continuation.is_none());
        let history = runtime.public_product_listing_history();
        assert!(Arc::ptr_eq(
            &history.get,
            &runtime.public_product_listing_history().get
        ));
        assert_eq!(0, runtime.postgres_pool().size());
        runtime.postgres_pool().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_transaction_failure_causes_for_both_composed_use_cases()
    -> Result<(), TestError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")?;
        let runtime = Runtime::from_pool(pool);
        runtime.postgres_pool().close().await;
        let family = runtime.public_listing_sources();
        let search = family
            .search
            .execute(&context(), search(None)?)
            .await
            .unwrap_err();
        let detail = family
            .get_by_slug
            .execute(
                &context(),
                GetPublicListingSourceBySlugRequest {
                    slug_id: ListingSourceSlugId::raw("fixture-source")?,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            search,
            SearchPublicListingSourcesError::BeginTransaction { .. }
        ));
        assert!(matches!(
            detail,
            GetPublicListingSourceBySlugError::BeginTransaction { .. }
        ));
        for error in [&search as &dyn Error, &detail as &dyn Error] {
            assert!(error.source().is_some_and(|cause| {
                cause
                    .source()
                    .is_some_and(|cause| cause.downcast_ref::<sqlx::Error>().is_some())
            }));
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reuse_listing_family_and_shipping_handles_without_provider_io()
    -> Result<(), TestError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")?;
        let runtime = Runtime::from_pool(pool);
        let dependencies = || -> Result<ProductListingDependencies, TestError> {
            Ok(ProductListingDependencies {
            opensearch: opensearch::OpenSearch::new(
                opensearch::http::transport::Transport::single_node("http://localhost:9")?,
            ),
            embeddings: Arc::new(UnusedEmbeddings),
            fx_cache: fxrate_service::readers::FxSearchCacheConfig::new(
                true,
                64,
                std::time::Duration::from_secs(10),
            )?,
            source_cache: product_listing_service::readers::SourceSearchCacheConfig::new(
                true,
                64,
                65_536,
                std::time::Duration::from_secs(10),
            )?,
            read_execution_policy: product_listing_service::use_cases::ProductListingSearchReadExecutionPolicy::Sequential,
        })
        };
        let first = runtime.public_product_listings(dependencies()?);
        let second = runtime.public_product_listings(dependencies()?);
        assert!(Arc::ptr_eq(&first.get, &second.get));
        assert!(Arc::ptr_eq(&first.search, &second.search));
        assert!(Arc::ptr_eq(&first.similar, &second.similar));
        assert!(std::ptr::eq(
            runtime.listing_shipping(),
            runtime.listing_shipping()
        ));
        Ok(())
    }

    struct UnusedEmbeddings;
    #[async_trait::async_trait]
    impl embedding::EmbeddingGenerator for UnusedEmbeddings {
        async fn embed_product(
            &self,
            _title: &embedding::EmbeddingText,
            _additional_text: Option<&embedding::EmbeddingText>,
            _image_url: Option<&embedding::EmbeddingImageUrl>,
        ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
            panic!("composing listing handles must not invoke embedding providers")
        }
        async fn embed_search_query(
            &self,
            _query: &embedding::EmbeddingText,
        ) -> Result<embedding::EmbeddingVector, embedding::EmbeddingError> {
            panic!("composing listing handles must not invoke embedding providers")
        }
    }
}
