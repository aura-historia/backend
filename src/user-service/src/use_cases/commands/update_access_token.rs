use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, Scope};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: PatchField<AccessTokenName>,
    pub scopes: PatchField<HashSet<Scope>>,
    pub expires: PatchField<OffsetDateTime>,
}

impl UpdateAccessTokenCommand {
    pub fn is_empty(&self) -> bool {
        !self.name.is_changed() && !self.scopes.is_changed() && !self.expires.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateAccessTokenError {}

#[async_trait::async_trait]
pub trait UpdateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAccessTokenCommand,
    ) -> Result<UpdateAccessTokenResult, UpdateAccessTokenError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateAccessTokenCommand {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_expires_cleared() {
        let command = UpdateAccessTokenCommand {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            expires: PatchField::Clear,
            ..Default::default()
        };

        assert!(!command.is_empty());
    }
}
