use crate::service::use_cases::queries::get_access_token::AccessTokenView;
use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensResult {
    pub items: Vec<AccessTokenView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListAccessTokensError {}

#[async_trait::async_trait]
pub trait ListAccessTokensUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAccessTokensRequest,
    ) -> Result<ListAccessTokensResult, ListAccessTokensError>;
}
