use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use common::actor::domain::Actor;
use common::error::boxed::BoxError;
use common::operation_context::{OperationContext, Principal};
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, RawAccessToken, Scope,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenCommand {
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub expires: Option<OffsetDateTime>,
    pub origin: AccessTokenOrigin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub raw_access_token: RawAccessToken,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAccessTokenError {
    #[error("authenticated actor required to create access token")]
    AuthenticatedActorRequired,
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
pub trait CreateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError>;
}

pub struct CreateAccessTokenHandler<S> {
    store: S,
}

impl<S> CreateAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> CreateAccessTokenUseCase for CreateAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "create_access_token",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError> {
        let actor = actor_from_context(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(actor));

        let raw_access_token = RawAccessToken::new();
        let now = OffsetDateTime::now_utc();
        let access_token = AccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw_access_token.clone().into(),
            user_id: command.user_id,
            name: command.name,
            scopes: command.scopes,
            origin: command.origin,
            expires: command.expires,
            created_by: actor,
            updated_by: actor,
            created: now,
            updated: now,
        };

        self.store.insert(access_token.clone()).await?;

        tracing::info!(
            event = "access_token.created",
            actor_id = %actor,
            user_id = %access_token.user_id,
            access_token_id = %access_token.id,
            outcome = "success",
        );

        Ok(CreateAccessTokenResult {
            user_id: access_token.user_id,
            access_token_id: access_token.id,
            raw_access_token,
        })
    }
}

fn actor_from_context(context: &OperationContext) -> Result<Actor, CreateAccessTokenError> {
    match &context.principal {
        Principal::Anonymous => Err(CreateAccessTokenError::AuthenticatedActorRequired),
        Principal::User(user_id) => Ok(Actor::User(*user_id)),
        Principal::Service(_) | Principal::System => Ok(Actor::System),
    }
}

impl From<AccessTokenStoreError> for CreateAccessTokenError {
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
