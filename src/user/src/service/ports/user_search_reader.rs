#![allow(dead_code)]

use crate::service::use_cases::queries::search_users::{SearchUsersRequest, SearchUsersResult};

#[derive(Debug, thiserror::Error)]
pub enum UserSearchReadError {
    #[error("temporary user search failure")]
    TemporarilyUnavailable,
    #[error("invalid user search read model")]
    InvalidReadModel,
    #[error("internal user search failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait UserSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchUsersRequest,
    ) -> Result<SearchUsersResult, UserSearchReadError>;
}
