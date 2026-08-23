#![allow(dead_code)]

use crate::use_cases::queries::search_users::{SearchUsersRequest, SearchUsersResult};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum UserSearchReadError {
    #[error("temporary user search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserSearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchUsersRequest,
    ) -> Result<SearchUsersResult, UserSearchReadError>;
}

pub trait UserSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserSearchReader + 'tx;
}
