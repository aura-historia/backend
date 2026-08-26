use crate::auth::core::{
    AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
};
use application::operation_context::CredentialCapability;
use std::collections::{BTreeSet, HashSet};
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
                },
            )
            .await
            .map_err(map_access_token_error)?;

        Ok(TransportPrincipal::User {
            user_id: result.user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: capabilities_from_scopes(&result.scopes),
        })
    }
}

fn map_access_token_error(error: AuthenticateAccessTokenError) -> AuthError {
    match error {
        AuthenticateAccessTokenError::NotFound | AuthenticateAccessTokenError::Expired => {
            AuthError::InvalidCredentials
        }
        AuthenticateAccessTokenError::TemporarilyUnavailable { .. } => {
            AuthError::TemporarilyUnavailable
        }
        AuthenticateAccessTokenError::Conflict { .. }
        | AuthenticateAccessTokenError::InvalidPersistedState { .. }
        | AuthenticateAccessTokenError::Internal { .. } => AuthError::Internal(error.to_string()),
    }
}

fn capabilities_from_scopes(scopes: &HashSet<Scope>) -> BTreeSet<CredentialCapability> {
    scopes.iter().copied().map(credential_capability).collect()
}

fn credential_capability(scope: Scope) -> CredentialCapability {
    match scope {
        Scope::ProductListingsWrite => CredentialCapability::ProductListingsWrite,
        Scope::ShopsRead => CredentialCapability::ShopsRead,
        Scope::ShopsWrite => CredentialCapability::ShopsWrite,
        Scope::PartnerShopApplicationsWrite => CredentialCapability::PartnerShopApplicationsWrite,
        Scope::PartnerShopsRead => CredentialCapability::PartnerShopsRead,
        Scope::PartnerShopsWrite => CredentialCapability::PartnerShopsWrite,
        Scope::UsersRead => CredentialCapability::UsersRead,
        Scope::UsersWrite => CredentialCapability::UsersWrite,
        Scope::AccessTokensRead => CredentialCapability::AccessTokensRead,
        Scope::AccessTokensWrite => CredentialCapability::AccessTokensWrite,
        Scope::SearchFiltersWrite => CredentialCapability::SearchFiltersWrite,
        Scope::WatchlistRead => CredentialCapability::WatchlistRead,
        Scope::WatchlistWrite => CredentialCapability::WatchlistWrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::{BoxError, box_error};
    use application::operation_context::{OperationContext, Principal};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::HashedRawAccessToken;
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

    #[test]
    fn should_map_all_token_scopes_to_credential_capabilities() {
        let cases = [
            (
                Scope::ProductListingsWrite,
                CredentialCapability::ProductListingsWrite,
            ),
            (Scope::ShopsRead, CredentialCapability::ShopsRead),
            (Scope::ShopsWrite, CredentialCapability::ShopsWrite),
            (
                Scope::PartnerShopApplicationsWrite,
                CredentialCapability::PartnerShopApplicationsWrite,
            ),
            (
                Scope::PartnerShopsRead,
                CredentialCapability::PartnerShopsRead,
            ),
            (
                Scope::PartnerShopsWrite,
                CredentialCapability::PartnerShopsWrite,
            ),
            (Scope::UsersRead, CredentialCapability::UsersRead),
            (Scope::UsersWrite, CredentialCapability::UsersWrite),
            (
                Scope::AccessTokensRead,
                CredentialCapability::AccessTokensRead,
            ),
            (
                Scope::AccessTokensWrite,
                CredentialCapability::AccessTokensWrite,
            ),
            (
                Scope::SearchFiltersWrite,
                CredentialCapability::SearchFiltersWrite,
            ),
            (Scope::WatchlistRead, CredentialCapability::WatchlistRead),
            (Scope::WatchlistWrite, CredentialCapability::WatchlistWrite),
        ];

        for (scope, capability) in cases {
            assert_eq!(capability, credential_capability(scope));
        }
        let scopes = cases.iter().map(|(scope, _)| *scope).collect();
        let capabilities = cases
            .iter()
            .map(|(_, capability)| *capability)
            .collect::<BTreeSet<_>>();
        assert_eq!(capabilities, capabilities_from_scopes(&scopes));
    }

    #[tokio::test]
    async fn should_map_temporary_access_token_failure() {
        let (use_case, _calls) = use_case(FakeTokenOutcome::TemporarilyUnavailable);
        let authenticator = AuraAccessTokenAuthenticator::new(use_case);
        let token = String::from(RawAccessToken::new());

        let result = authenticator.authenticate(&token, &metadata()).await;

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

            let result = authenticator.authenticate(&token, &metadata()).await;

            assert!(matches!(result, Err(AuthError::Internal(_))));
        }
    }
}
