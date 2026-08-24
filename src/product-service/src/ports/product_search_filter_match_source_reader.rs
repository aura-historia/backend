use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_core::{
    description::Description,
    product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation},
    product_id::ProductId,
    product_image::ProductImage,
    product_lifecycle::ProductLifecycle,
    product_slug_id::ProductSlugId,
    product_state::ProductState,
    shops_product_id::ShopsProductId,
    title::Title,
};
use shop_core::seller_slug_id::SellerSlugId;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use shop_core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

pub use shop_core::shop_type::ShopType as ProductSearchFilterMatchShopType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductSearchFilterMatchSourceEventKind {
    Domain,
    Enrichment,
    Ignored,
}

impl ProductSearchFilterMatchSourceEventKind {
    pub fn is_percolation_trigger(self) -> bool {
        matches!(self, Self::Domain | Self::Enrichment)
    }
}

/// Exact immutable Product event reference for batched source reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductSearchFilterMatchSourceRef {
    pub product_id: ProductId,
    pub event_id: EventId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSearchFilterMatchSource {
    /// Stable identifier of the source Product event.
    pub event_id: EventId,
    /// Whether this event type is routed to search-filter percolation.
    pub event_kind: ProductSearchFilterMatchSourceEventKind,
    /// Immutable occurrence time from `product_events.event_time`.
    pub origin_event_time: OffsetDateTime,
    /// Current Product event identity. This rejects stale CDC triggers.
    pub current_event_id: EventId,
    /// Monotonic authoritative version for external OpenSearch writes.
    pub projection_version: i64,
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub seller_id: ShopId,
    pub seller_slug_id: SellerSlugId,
    pub seller_name: ShopName,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    /// Translations take precedence over the original text for the same language.
    pub titles: HashMap<Language, Title>,
    /// Translations take precedence over the original text for the same language.
    pub descriptions: HashMap<Language, Description>,
    /// Native persisted prices.
    pub pricing: ProductPricing,
    /// Immutable valuation captured when the Product was sold.
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub image: Option<ProductImage>,
    pub images: IndexSet<ProductImage>,
    /// Authoritative embedding, when enrichment completed.
    pub embedding: Option<Vec<f32>>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchFilterMatchSourceReadError {
    #[error("product search-filter match source query failed")]
    QueryFailed {
        #[source]
        source: application::error::BoxError,
    },
    #[error("product search-filter match source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: application::error::BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductSearchFilterMatchSourceReader: Send {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>;

    /// Reads exact Product event sources in one batch. Missing refs are absent.
    async fn find_sources(
        &mut self,
        refs: &[ProductSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductSearchFilterMatchSourceRef, ProductSearchFilterMatchSource>,
        ProductSearchFilterMatchSourceReadError,
    > {
        let mut sources = HashMap::new();
        for reference in refs {
            if let Some(source) = self
                .find_source(reference.event_id, reference.product_id)
                .await?
            {
                sources.insert(*reference, source);
            }
        }
        Ok(sources)
    }
}

pub trait ProductSearchFilterMatchSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductSearchFilterMatchSourceReader + 'tx;
}
