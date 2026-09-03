#![allow(dead_code)]

use crate::use_cases::queries::search_parties::{SearchPartiesRequest, SearchPartiesResult};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum PartySearchReadError {
    #[error("temporary party search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid party search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal party search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartySearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchPartiesRequest,
    ) -> Result<SearchPartiesResult, PartySearchReadError>;
}

pub trait PartySearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartySearchReader + 'tx;
}
