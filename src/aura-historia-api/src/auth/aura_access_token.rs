use crate::auth::core::{
    AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
};
use aura_historia_auth::{AccessTokenAuthenticationError, authenticate_access_token};
use user_service::use_cases::AuthenticateAccessTokenUseCase;
#[cfg(test)]
use user_service::use_cases::{AuthenticateAccessTokenError, AuthenticateAccessTokenRequest};

pub struct AuraAccessTokenAuthenticator<U> {
    use_case: U,
}

impl<U> AuraAccessTokenAuthenticator<U> {
    pub fn new(use_case: U) -> Self {
        Self { use_case }
    }
}

#[async_trait::async_trait]
impl<U: AuthenticateAccessTokenUseCase> TokenAuthenticator for AuraAccessTokenAuthenticator<U> {
    async fn authenticate(
        &self,
        bearer_token: &str,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let context = TransportPrincipal::Anonymous.operation_context(metadata.clone());
        let identity = authenticate_access_token(&self.use_case, bearer_token, &context)
            .await
            .map_err(map_access_token_error)?;
        Ok(TransportPrincipal::User {
            user_id: identity.user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: identity.capabilities,
        })
    }
}

fn map_access_token_error(error: AccessTokenAuthenticationError) -> AuthError {
    match error {
        AccessTokenAuthenticationError::MalformedCredentials { .. } => {
            AuthError::MalformedCredentials
        }
        AccessTokenAuthenticationError::InvalidCredentials => AuthError::InvalidCredentials,
        AccessTokenAuthenticationError::TemporarilyUnavailable { .. }
        | AccessTokenAuthenticationError::Internal { .. } => AuthError::AuraAccessToken(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::{BoxError, box_error};
    use application::operation_context::CredentialCapability;
    use application::operation_context::{OperationContext, Principal};
    use std::collections::{BTreeSet, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{HashedRawAccessToken, RawAccessToken, Scope};
    use user_core::user_id::UserId;
    use user_service::use_cases::AuthenticateAccessTokenResult;

    #[derive(Clone)]
    enum FakeTokenOutcome {
        Success {
            user_id: UserId,
            scopes: HashSet<Scope>,
        },
        NotFound,
        Expired,
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
                FakeTokenOutcome::Success {
                    user_id,
                    ref scopes,
                } => Ok(AuthenticateAccessTokenResult {
                    user_id,
                    scopes: scopes.clone(),
                }),
                FakeTokenOutcome::NotFound => Err(AuthenticateAccessTokenError::NotFound),
                FakeTokenOutcome::Expired => Err(AuthenticateAccessTokenError::Expired),

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

    fn product_listings_write_scope() -> HashSet<Scope> {
        HashSet::from([Scope::ProductListingsWrite])
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
        let (use_case, calls) = use_case(FakeTokenOutcome::Success {
            user_id,
            scopes: product_listings_write_scope(),
        });
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);

        let principal = authenticator.authenticate(&token, &metadata()).await?;

        let recorded = lock(&calls).clone();
        let expected_hash: HashedRawAccessToken = raw_token.into();
        assert!(matches!(
            principal,
            TransportPrincipal::User {
                user_id: actual,
                auth_method: AuthMethod::AuraAccessToken,
                capabilities,
            } if actual == user_id
                && capabilities == BTreeSet::from([CredentialCapability::ProductListingsWrite])
        ));
        assert_eq!(1, recorded.len());
        assert_eq!(Principal::Anonymous, recorded[0].0.principal);
        assert_eq!("req-1", recorded[0].0.request_id.as_str());
        assert_eq!("corr-1", recorded[0].0.correlation_id.as_str());
        assert_eq!(expected_hash, recorded[0].1.hashed_token);
        Ok(())
    }

    #[tokio::test]
    async fn should_authenticate_auction_read_scope_as_a_delegated_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let token = String::from(RawAccessToken::new());
        let (use_case, _calls) = use_case(FakeTokenOutcome::Success {
            user_id,
            scopes: HashSet::from([Scope::AuctionsRead]),
        });
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);

        let principal = authenticator.authenticate(&token, &metadata()).await?;

        assert_eq!(
            TransportPrincipal::User {
                user_id,
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::AuctionsRead]),
            },
            principal
        );
        assert_eq!(
            Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::AuctionsRead]),
            },
            principal.operation_context(metadata()).principal
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_malformed() {
        let (use_case, calls) = use_case(FakeTokenOutcome::Success {
            user_id: UserId::new(),
            scopes: HashSet::new(),
        });
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);

        let result = authenticator
            .authenticate("not-an-aura-token", &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        assert!(lock(&calls).is_empty());
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_revoked() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::NotFound);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn should_reject_opaque_access_token_when_expired() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::Expired);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn should_map_temporary_access_token_failure() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::TemporarilyUnavailable);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(
            result,
            Err(AuthError::AuraAccessToken(
                AccessTokenAuthenticationError::TemporarilyUnavailable { .. }
            ))
        ));
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

            let result = authenticator.authenticate(&token, &metadata()).await;

            assert!(matches!(
                result,
                Err(AuthError::AuraAccessToken(
                    AccessTokenAuthenticationError::Internal { .. }
                ))
            ));
        }
    }

    #[tokio::test]
    async fn should_preserve_technical_causes_through_rest_authentication_and_error_mapping() {
        use crate::error::{AUTH_INTERNAL_ERROR, AUTH_TEMPORARILY_UNAVAILABLE, ApiError};
        use axum::{http::StatusCode, response::IntoResponse};
        use std::error::Error;

        for (outcome, status, code) in [
            (
                FakeTokenOutcome::TemporarilyUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                AUTH_TEMPORARILY_UNAVAILABLE,
            ),
            (
                FakeTokenOutcome::Conflict,
                StatusCode::INTERNAL_SERVER_ERROR,
                AUTH_INTERNAL_ERROR,
            ),
            (
                FakeTokenOutcome::InvalidPersistedState,
                StatusCode::INTERNAL_SERVER_ERROR,
                AUTH_INTERNAL_ERROR,
            ),
            (
                FakeTokenOutcome::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                AUTH_INTERNAL_ERROR,
            ),
        ] {
            let (use_case, _) = use_case(outcome);
            let authenticator = AuraAccessTokenAuthenticator::new(use_case);
            let token = String::from(RawAccessToken::new());
            let error = authenticator
                .authenticate(&token, &metadata())
                .await
                .unwrap_err();
            let error = ApiError::from(error);
            let mut source = error.source().expect("authentication error source");
            while let Some(cause) = source.source() {
                source = cause;
            }
            assert!(source.is::<std::io::Error>());
            assert_eq!("boom", source.to_string());
            assert!(!error.to_string().contains("boom"));

            let response = error.into_response();
            assert_eq!(status, response.status());
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(code.to_string(), body["error"]);
            assert!(!String::from_utf8_lossy(&bytes).contains("boom"));
            assert!(!String::from_utf8_lossy(&bytes).contains(&token));
        }
    }
}
