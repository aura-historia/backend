#![allow(dead_code)]

use crate::service::use_cases::queries::get_shop::{GetShopRequest, ShopDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum ShopDetailsReadError {
    #[error("temporary shop details read failure")]
    TemporarilyUnavailable,
    #[error("invalid shop details read model")]
    InvalidReadModel,
    #[error("internal shop details read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ShopDetailsReader: Send + Sync {
    async fn find_details(
        &self,
        request: &GetShopRequest,
    ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError>;
}
