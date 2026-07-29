#![allow(dead_code)]

use crate::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};
use common::error::boxed::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum UserAdminReadError {
    #[error("temporary user admin read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user admin read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user admin read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserAdminReader: Send {
    async fn find_admin_view(
        &mut self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAdminReadError>;
}

pub trait UserAdminReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserAdminReader + 'tx;
}
