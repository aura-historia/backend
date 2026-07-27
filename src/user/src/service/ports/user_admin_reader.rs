#![allow(dead_code)]

use crate::service::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum UserAdminReadError {
    #[error("temporary user admin read failure")]
    TemporarilyUnavailable,
    #[error("invalid user admin read model")]
    InvalidReadModel,
    #[error("internal user admin read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait UserAdminReader: Send + Sync {
    async fn find_admin_view(
        &self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAdminReadError>;
}
