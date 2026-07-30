use crate::auth::core::{
    AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
};
use std::collections::HashSet;
use user_core::access_token::{RawAccessToken, Scope};
use user_service::use_cases::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenRequest, AuthenticateAccessTokenUseCase,
};

pub struct AuraAccessTokenAuthenticator<U> {
    use_case: U,
}

impl<U> AuraAccessTokenAuthenticator<U> {
    pub fn new(use_case: U) -> Self {
        Self { use_case }
    }
}

#[async_trait::async_trait]
impl<U> TokenAuthenticator for AuraAccessTokenAuthenticator<U>
where
    U: AuthenticateAccessTokenUseCase,
{
    async fn authenticate(
        &self,
        bearer_token: &str,
        required_scopes: &HashSet<Scope>,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let raw_token = RawAccessToken::try_from(bearer_token.to_owned())
            .map_err(|_| AuthError::MalformedCredentials)?;
        let context = TransportPrincipal::Anonymous.operation_context(metadata.clone());
        let result = self
            .use_case
            .execute(
                &context,
                AuthenticateAccessTokenRequest {
                    hashed_token: raw_token.into(),
                    required_scopes: required_scopes.clone(),
                },
            )
            .await
            .map_err(map_access_token_error)?;

        Ok(TransportPrincipal::User {
            user_id: result.user_id,
            auth_method: AuthMethod::AuraAccessToken,
            scopes: required_scopes.clone(),
        })
    }
}

fn map_access_token_error(error: AuthenticateAccessTokenError) -> AuthError {
    match error {
        AuthenticateAccessTokenError::NotFound | AuthenticateAccessTokenError::Expired => {
            AuthError::InvalidCredentials
        }
        AuthenticateAccessTokenError::InsufficientScope => AuthError::InsufficientScope,
        AuthenticateAccessTokenError::TemporarilyUnavailable { .. } => {
            AuthError::TemporarilyUnavailable
        }
        AuthenticateAccessTokenError::Conflict { .. }
        | AuthenticateAccessTokenError::InvalidPersistedState { .. }
        | AuthenticateAccessTokenError::Internal { .. } => AuthError::Internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::error::boxed::{BoxError, box_error};
    use common::operation_context::{OperationContext, Principal};
    use common::user_id::UserId;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::HashedRawAccessToken;
    use user_service::use_cases::AuthenticateAccessTokenResult;

    #[derive(Clone)]
    enum FakeTokenOutcome {
        Success(UserId),
        NotFound,
        Expired,
        InsufficientScope,
        Conflict,
        TemporarilyUnavailable,
        InvalidPersistedState,
        Internal,
    }

    type AccessTokenCall = (OperationContext, AuthenticateAccessTokenRequest);
    type AccessTokenCalls = Arc<Mutex<Vec<AccessTokenCall>>>;

    #[derive(Clone)]
    struct FakeAccessTokenUseCase {
        outcome: FakeTokenOutcome,
        calls: AccessTokenCalls,
    }

    #[async_trait::async_trait]
    impl AuthenticateAccessTokenUseCase for FakeAccessTokenUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: AuthenticateAccessTokenRequest,
        ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
            lock(&self.calls).push((context.clone(), request));
            match self.outcome {
                FakeTokenOutcome::Success(user_id) => Ok(AuthenticateAccessTokenResult { user_id }),
                FakeTokenOutcome::NotFound => Err(AuthenticateAccessTokenError::NotFound),
                FakeTokenOutcome::Expired => Err(AuthenticateAccessTokenError::Expired),
                FakeTokenOutcome::InsufficientScope => {
                    Err(AuthenticateAccessTokenError::InsufficientScope)
                }
                FakeTokenOutcome::Conflict => {
                    Err(AuthenticateAccessTokenError::Conflict { source: boxed() })
                }
                FakeTokenOutcome::TemporarilyUnavailable => {
                    Err(AuthenticateAccessTokenError::TemporarilyUnavailable { source: boxed() })
                }
                FakeTokenOutcome::InvalidPersistedState => {
                    Err(AuthenticateAccessTokenError::InvalidPersistedState { source: boxed() })
                }
                FakeTokenOutcome::Internal => {
                    Err(AuthenticateAccessTokenError::Internal { source: boxed() })
                }
            }
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn metadata() -> RequestMetadata {
        RequestMetadata::new("req-1", "corr-1")
    }

    fn required_products_write() -> HashSet<Scope> {
        HashSet::from([Scope::ProductsWrite])
    }

    fn use_case(outcome: FakeTokenOutcome) -> (FakeAccessTokenUseCase, AccessTokenCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            FakeAccessTokenUseCase {
                outcome,
                calls: calls.clone(),
            },
            calls,
        )
    }

    #[tokio::test]
    async fn should_authenticate_opaque_access_token_when_use_case_accepts()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let raw_token = RawAccessToken::new();
        let token = String::from(raw_token.clone());
        let (use_case, calls) = use_case(FakeTokenOutcome::Success(user_id));
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);

        let principal = authenticator
            .authenticate(&token, &required_products_write(), &metadata())
            .await?;

        let recorded = lock(&calls).clone();
        let expected_hash: HashedRawAccessToken = raw_token.into();
        assert!(matches!(
            principal,
            TransportPrincipal::User {
                user_id: actual,
                auth_method: AuthMethod::AuraAccessToken,
                scopes,
            } if actual == user_id && scopes == required_products_write()
        ));
        assert_eq!(1, recorded.len());
        assert_eq!(Principal::Anonymous, recorded[0].0.principal);
        assert_eq!("req-1", recorded[0].0.request_id.as_str());
        assert_eq!("corr-1", recorded[0].0.correlation_id.as_str());
        assert_eq!(expected_hash, recorded[0].1.hashed_token);
        assert_eq!(required_products_write(), recorded[0].1.required_scopes);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_malformed() {
        let (use_case, calls) = use_case(FakeTokenOutcome::Success(UserId::new()));
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);

        let result = authenticator
            .authenticate("not-an-aura-token", &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        assert!(lock(&calls).is_empty());
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_revoked() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::NotFound);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_expired() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::Expired);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_scope_missing() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::InsufficientScope);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator
            .authenticate(&token, &required_products_write(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InsufficientScope)));
    }

    #[tokio::test]
    async fn should_map_temporary_access_token_failure() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::TemporarilyUnavailable);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::TemporarilyUnavailable)));
    }

    #[tokio::test]
    async fn should_map_internal_access_token_failures() {
        for outcome in [
            FakeTokenOutcome::Conflict,
            FakeTokenOutcome::InvalidPersistedState,
            FakeTokenOutcome::Internal,
        ] {
            let (use_case, _calls) = use_case(outcome);
            let authenticator = AuraAccessTokenAuthenticator::new(use_case);
            let token = String::from(RawAccessToken::new());

            let result = authenticator
                .authenticate(&token, &HashSet::new(), &metadata())
                .await;

            assert!(matches!(result, Err(AuthError::Internal(_))));
        }
    }
}
