use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use common::actor::domain::Actor;
use common::error::boxed::BoxError;
use common::operation_context::{OperationContext, Principal};
use common::patch_field::PatchField;
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessToken, AccessTokenId, AccessTokenName, Scope};

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
pub enum UpdateAccessTokenError {
    #[error("authenticated actor required to update access token")]
    AuthenticatedActorRequired,
    #[error("access token not found")]
    AccessTokenNotFound,
    #[error("access token name is required")]
    NameRequired,
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token store failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UpdateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAccessTokenCommand,
    ) -> Result<UpdateAccessTokenResult, UpdateAccessTokenError>;
}

pub struct UpdateAccessTokenHandler<S> {
    store: S,
}

impl<S> UpdateAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> UpdateAccessTokenUseCase for UpdateAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "update_access_token",
        skip_all,
        fields(
            user_id = %command.user_id,
            access_token_id = %command.access_token_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAccessTokenCommand,
    ) -> Result<UpdateAccessTokenResult, UpdateAccessTokenError> {
        let actor = actor_from_context(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(actor));

        let mut access_token = self
            .store
            .find_by_id(&command.user_id, &command.access_token_id)
            .await?
            .ok_or(UpdateAccessTokenError::AccessTokenNotFound)?;

        let changed = apply_update(&mut access_token, command, actor)?;
        if changed {
            self.store.replace(access_token.clone()).await?;
        }

        tracing::info!(
            event = "access_token.updated",
            actor_id = %actor,
            user_id = %access_token.user_id,
            access_token_id = %access_token.id,
            changed,
            outcome = "success",
        );

        Ok(UpdateAccessTokenResult {
            user_id: access_token.user_id,
            access_token_id: access_token.id,
        })
    }
}

fn apply_update(
    access_token: &mut AccessToken,
    command: UpdateAccessTokenCommand,
    actor: Actor,
) -> Result<bool, UpdateAccessTokenError> {
    let mut changed = false;

    match command.name {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.name != value;
            access_token.name = value;
        }
        PatchField::Clear => return Err(UpdateAccessTokenError::NameRequired),
    }
    match command.scopes {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.scopes != value;
            access_token.scopes = value;
        }
        PatchField::Clear => {
            changed |= !access_token.scopes.is_empty();
            access_token.scopes.clear();
        }
    }
    match command.expires {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.expires != Some(value);
            access_token.expires = Some(value);
        }
        PatchField::Clear => {
            changed |= access_token.expires.is_some();
            access_token.expires = None;
        }
    }

    if changed {
        access_token.updated_by = actor;
        access_token.updated = OffsetDateTime::now_utc();
    }

    Ok(changed)
}

fn actor_from_context(context: &OperationContext) -> Result<Actor, UpdateAccessTokenError> {
    match &context.principal {
        Principal::Anonymous => Err(UpdateAccessTokenError::AuthenticatedActorRequired),
        Principal::User(user_id) => Ok(Actor::User(*user_id)),
        Principal::Service(_) | Principal::System => Ok(Actor::System),
    }
}

impl From<AccessTokenStoreError> for UpdateAccessTokenError {
    fn from(error: AccessTokenStoreError) -> Self {
        match error {
            AccessTokenStoreError::Conflict { source } => Self::Conflict { source },
            AccessTokenStoreError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenStoreError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenStoreError::Internal { source } => Self::Internal { source },
        }
    }
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
