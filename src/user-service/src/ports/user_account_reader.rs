#![allow(dead_code)]

use crate::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};
use common::error::boxed::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum UserAccountReadError {
    #[error("temporary user account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user account read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user account read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserAccountReader: Send {
    async fn find_account(
        &mut self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAccountReadError>;
}

pub trait UserAccountReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserAccountReader + 'tx;
}
