#![allow(dead_code)]

use crate::use_cases::queries::get_shop::{GetShopRequest, ShopDetailsView};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum ShopDetailsReadError {
    #[error("temporary shop details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid shop details read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal shop details read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ShopDetailsReader: Send {
    async fn find_details(
        &mut self,
        request: &GetShopRequest,
    ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError>;
}

pub trait ShopDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShopDetailsReader + 'tx;
}
