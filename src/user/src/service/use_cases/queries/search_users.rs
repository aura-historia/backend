use crate::core::{role::UserRole, tier::UserTier, user_search::UserSearch};
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersRequest {
    pub search: UserSearch,
    pub sort: Option<Sort<crate::core::sort_user_field::SortUserField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<crate::core::first_name::FirstName>,
    pub last_name: Option<crate::core::last_name::LastName>,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersResult {
    pub items: Vec<UserSummary>,
    pub cursor: Cursor<Value>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchUsersError {}

#[async_trait::async_trait]
pub trait SearchUsersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError>;
}
