#![allow(dead_code)]

use crate::use_cases::list_partner_shops::{ListPartnerShopsRequest, ListPartnerShopsResult};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum UserPartnerShopsReadError {
    #[error("temporary user partner shops read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user partner shops read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user partner shops read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserPartnerShopsReader: Send {
    async fn list_partner_shops(
        &mut self,
        request: &ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, UserPartnerShopsReadError>;
}

pub trait UserPartnerShopsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserPartnerShopsReader + 'tx;
}
