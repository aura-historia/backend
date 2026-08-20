use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use application::operation_context::{
    AuthenticationRequired, CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::error::boxed::BoxError;
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
    pub view: crate::use_cases::queries::get_access_token::AccessTokenView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateAccessTokenError {
    #[error("authenticated actor required to update access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
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
        authorize_access_token_write(context, command.user_id)?;
        let principal = context.principal.require_authenticated()?.clone();
        tracing::Span::current().record("actor_id", tracing::field::display(principal.label()));

        let mut access_token = self
            .store
            .find_by_id(&command.user_id, &command.access_token_id)
            .await?
            .ok_or(UpdateAccessTokenError::AccessTokenNotFound)?;

        let changed = apply_update(&mut access_token, command)?;
        if changed {
            self.store.replace(access_token.clone()).await?;
        }

        tracing::info!(
            event = "access_token.updated",
            actor_id = %principal.label(),
            user_id = %access_token.user_id(),
            access_token_id = %access_token.id(),
            changed,
            outcome = "success",
        );

        Ok(UpdateAccessTokenResult {
            view: crate::use_cases::queries::get_access_token::AccessTokenView::from(access_token),
        })
    }
}

fn apply_update(
    access_token: &mut AccessToken,
    command: UpdateAccessTokenCommand,
) -> Result<bool, UpdateAccessTokenError> {
    let mut changed = false;

    match command.name {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.change_name(value);
        }
        PatchField::Clear => return Err(UpdateAccessTokenError::NameRequired),
    }
    match command.scopes {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.replace_scopes(value);
        }
        PatchField::Clear => {
            changed |= access_token.replace_scopes(HashSet::new());
        }
    }
    match command.expires {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.change_expires(Some(value));
        }
        PatchField::Clear => {
            changed |= access_token.change_expires(None);
        }
    }

    Ok(changed)
}

impl From<OperationAuthorizationError> for UpdateAccessTokenError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<AuthenticationRequired> for UpdateAccessTokenError {
    fn from(_: AuthenticationRequired) -> Self {
        Self::AuthenticatedActorRequired
    }
}

fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateAccessTokenError>()
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
    #![allow(dead_code, unused_imports)]
    use super::{
        UpdateAccessTokenCommand, UpdateAccessTokenError, UpdateAccessTokenHandler,
        UpdateAccessTokenUseCase,
    };
    use common::user_id::UserId;

    use crate::ports::{AccessTokenStore, AccessTokenStoreError};
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use common::error::boxed::{BoxError, box_error};
    use common::patch_field::PatchField;
    use std::collections::{BTreeSet, HashSet};
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::{Duration, OffsetDateTime};
    use user_core::access_token::{
        AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
        NewAccessToken, RawAccessToken, Scope,
    };

    #[derive(Debug, Clone, Copy)]
    enum StoreErrorKind {
        Conflict,
        TemporarilyUnavailable,
        InvalidPersistedState,
        Internal,
    }

    #[derive(Default)]
    struct StoreState {
        token: Option<AccessToken>,
        tokens: Vec<AccessToken>,
        find_by_id_error: Option<StoreErrorKind>,
        find_by_hashed_error: Option<StoreErrorKind>,
        list_error: Option<StoreErrorKind>,
        insert_error: Option<StoreErrorKind>,
        replace_error: Option<StoreErrorKind>,
        delete_error: Option<StoreErrorKind>,
        find_by_id_calls: usize,
        find_by_hashed_calls: usize,
        list_calls: usize,
        insert_calls: usize,
        replace_calls: usize,
        delete_calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeAccessTokenStore {
        state: Arc<Mutex<StoreState>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn ctx(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn token(
        user_id: common::user_id::UserId,
        scopes: HashSet<Scope>,
        expires: Option<OffsetDateTime>,
    ) -> AccessToken {
        let raw = RawAccessToken::new();
        AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.into(),
            user_id,
            name: AccessTokenName::from("test token"),
            scopes,
            origin: AccessTokenOrigin::User,
            expires,
        })
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn store_error(kind: StoreErrorKind) -> AccessTokenStoreError {
        match kind {
            StoreErrorKind::Conflict => AccessTokenStoreError::Conflict { source: boxed() },
            StoreErrorKind::TemporarilyUnavailable => {
                AccessTokenStoreError::TemporarilyUnavailable { source: boxed() }
            }
            StoreErrorKind::InvalidPersistedState => {
                AccessTokenStoreError::InvalidPersistedState { source: boxed() }
            }
            StoreErrorKind::Internal => AccessTokenStoreError::Internal { source: boxed() },
        }
    }

    fn assert_error<T, E, F>(result: Result<T, E>, predicate: F)
    where
        E: Debug,
        F: FnOnce(&E) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    fn assert_ok<T, E: Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected ok, got {error:?}"),
        }
    }

    #[async_trait::async_trait]
    impl AccessTokenStore for FakeAccessTokenStore {
        async fn find_by_id(
            &self,
            _user_id: &common::user_id::UserId,
            _access_token_id: &AccessTokenId,
        ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.find_by_id_calls += 1;
            if let Some(kind) = state.find_by_id_error {
                Err(store_error(kind))
            } else {
                Ok(state.token.clone())
            }
        }

        async fn find_by_hashed_token(
            &self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.find_by_hashed_calls += 1;
            if let Some(kind) = state.find_by_hashed_error {
                Err(store_error(kind))
            } else {
                Ok(state.token.clone())
            }
        }

        async fn list_for_user(
            &self,
            _user_id: &common::user_id::UserId,
        ) -> Result<Vec<AccessToken>, AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.list_calls += 1;
            if let Some(kind) = state.list_error {
                Err(store_error(kind))
            } else {
                Ok(state.tokens.clone())
            }
        }

        async fn insert(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            if let Some(kind) = state.insert_error {
                Err(store_error(kind))
            } else {
                state.token = Some(access_token);
                Ok(())
            }
        }

        async fn replace(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.replace_calls += 1;
            if let Some(kind) = state.replace_error {
                Err(store_error(kind))
            } else {
                state.token = Some(access_token);
                Ok(())
            }
        }

        async fn delete(
            &self,
            _user_id: &common::user_id::UserId,
            _access_token_id: &AccessTokenId,
        ) -> Result<(), AccessTokenStoreError> {
            let mut state = lock(&self.state);
            state.delete_calls += 1;
            if let Some(kind) = state.delete_error {
                Err(store_error(kind))
            } else {
                state.token = None;
                Ok(())
            }
        }
    }

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

    #[tokio::test]
    async fn should_update_access_token_and_skip_replace_when_noop() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();
        let current = token(user_id, HashSet::from([Scope::ShopsWrite]), None);
        lock(&store.state).token = Some(current.clone());

        assert_ok(
            UpdateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::Service("svc".to_owned())),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id: current.id(),
                        name: PatchField::Set(AccessTokenName::from("updated")),
                        scopes: PatchField::Clear,
                        expires: PatchField::Set(OffsetDateTime::now_utc() + Duration::days(1)),
                    },
                )
                .await,
        );
        assert_eq!(1, lock(&store.state).replace_calls);

        assert_ok(
            UpdateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::System),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id: current.id(),
                        ..Default::default()
                    },
                )
                .await,
        );
        assert_eq!(1, lock(&store.state).replace_calls);
    }

    #[tokio::test]
    async fn should_update_access_token_with_delegated_user() {
        let user_id = UserId::new();
        let store = FakeAccessTokenStore::default();
        let current = token(user_id, HashSet::new(), None);
        lock(&store.state).token = Some(current.clone());

        assert_ok(
            UpdateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::DelegatedUser {
                        user_id,
                        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
                    }),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id: current.id(),
                        name: PatchField::Set(AccessTokenName::from("delegated update")),
                        ..Default::default()
                    },
                )
                .await,
        );

        assert_eq!(1, lock(&store.state).replace_calls);
    }

    #[tokio::test]
    async fn should_handle_update_access_token_auth_not_found_and_name_required() {
        let user_id = UserId::new();
        let access_token_id = AccessTokenId::new();
        let store = FakeAccessTokenStore::default();
        assert_error(
            UpdateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::Anonymous),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateAccessTokenError::AuthenticatedActorRequired),
        );
        assert_error(
            UpdateAccessTokenHandler::new(store.clone())
                .execute(
                    &ctx(Principal::System),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateAccessTokenError::AccessTokenNotFound),
        );

        lock(&store.state).token = Some(token(user_id, HashSet::new(), None));
        assert_error(
            UpdateAccessTokenHandler::new(store)
                .execute(
                    &ctx(Principal::System),
                    UpdateAccessTokenCommand {
                        user_id,
                        access_token_id,
                        name: PatchField::Clear,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateAccessTokenError::NameRequired),
        );
    }

    #[tokio::test]
    async fn should_map_access_token_store_errors_for_update() {
        for kind in [
            StoreErrorKind::Conflict,
            StoreErrorKind::TemporarilyUnavailable,
            StoreErrorKind::InvalidPersistedState,
            StoreErrorKind::Internal,
        ] {
            let user_id = UserId::new();
            let store = FakeAccessTokenStore::default();
            lock(&store.state).find_by_id_error = Some(kind);
            assert_error(
                UpdateAccessTokenHandler::new(store)
                    .execute(
                        &ctx(Principal::System),
                        UpdateAccessTokenCommand {
                            user_id,
                            access_token_id: AccessTokenId::new(),
                            ..Default::default()
                        },
                    )
                    .await,
                |error| {
                    matches!(
                        error,
                        UpdateAccessTokenError::Conflict { .. }
                            | UpdateAccessTokenError::TemporarilyUnavailable { .. }
                            | UpdateAccessTokenError::InvalidPersistedState { .. }
                            | UpdateAccessTokenError::Internal { .. }
                    )
                },
            );
        }
    }
}
