#![allow(dead_code)]

use crate::service::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};

#[derive(Debug, thiserror::Error)]
pub enum UserAccountReadError {
    #[error("temporary user account read failure")]
    TemporarilyUnavailable,
    #[error("invalid user account read model")]
    InvalidReadModel,
    #[error("internal user account read failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait UserAccountReader: Send + Sync {
    async fn find_account(
        &self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAccountReadError>;
}
