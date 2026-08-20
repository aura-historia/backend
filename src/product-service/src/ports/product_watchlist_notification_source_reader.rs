use common::{error::boxed::BoxError, event_id::EventId};
use localization::Language;
use money::Price;
use product_core::{
    product_id::ProductId, product_image::ProductImage, product_slug_id::ProductSlugId,
    product_state::ProductState, shops_product_id::ShopsProductId, title::Title,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductWatchlistNotificationSource {
    pub event_id: EventId,
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_slug_id: ShopSlugId,
    pub shop_name: ShopName,
    pub title: Option<HashMap<Language, Title>>,
    pub image: Option<ProductImage>,
    pub url: Url,
    pub view_url: Url,
    pub change: ProductWatchlistNotificationChange,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductWatchlistNotificationChange {
    PriceChanged {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    StateChanged {
        old_state: ProductState,
        new_state: ProductState,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductWatchlistNotificationSourceReadError {
    #[error("watchlist notification source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist notification source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductWatchlistNotificationSourceReader: Send {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<
        Option<ProductWatchlistNotificationSource>,
        ProductWatchlistNotificationSourceReadError,
    >;
}

pub trait ProductWatchlistNotificationSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductWatchlistNotificationSourceReader + 'tx;
}
