use crate::auth::core::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};

pub struct ApiAuthService<J, A> {
    jwt: J,
    access_token: A,
}

impl<J, A> ApiAuthService<J, A> {
    pub fn new(jwt: J, access_token: A) -> Self {
        Self { jwt, access_token }
    }
}

#[async_trait::async_trait]
impl<J, A> TokenAuthenticator for ApiAuthService<J, A>
where
    J: TokenAuthenticator,
    A: TokenAuthenticator,
{
    async fn authenticate(
        &self,
        bearer_token: &str,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        if bearer_token.starts_with("aurahistoria_accesstoken_") {
            return self.access_token.authenticate(bearer_token, metadata).await;
        }
        if bearer_token.matches('.').count() == 2 {
            return self.jwt.authenticate(bearer_token, metadata).await;
        }
        Err(AuthError::MalformedCredentials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::core::AuthMethod;
    use application::operation_context::CredentialCapability;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum AuthenticatorKind {
        Jwt,
        AccessToken,
    }

    type AuthCall = (AuthenticatorKind, String, RequestMetadata);
    type AuthCalls = Arc<Mutex<Vec<AuthCall>>>;

    #[derive(Clone)]
    struct RecordingAuthenticator {
        kind: AuthenticatorKind,
        result: Result<TransportPrincipal, StaticAuthError>,
        calls: AuthCalls,
    }

    #[derive(Debug, Clone, Copy)]
    enum StaticAuthError {
        InvalidCredentials,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for RecordingAuthenticator {
        async fn authenticate(
            &self,
            bearer_token: &str,
            metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            lock(&self.calls).push((self.kind, bearer_token.to_owned(), metadata.clone()));
            self.result
                .clone()
                .map_err(|_| AuthError::InvalidCredentials)
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn metadata() -> RequestMetadata {
        RequestMetadata::new("req-1", "corr-1")
    }

    fn service(
        jwt_result: Result<TransportPrincipal, StaticAuthError>,
        access_result: Result<TransportPrincipal, StaticAuthError>,
    ) -> (
        ApiAuthService<RecordingAuthenticator, RecordingAuthenticator>,
        AuthCalls,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let jwt = RecordingAuthenticator {
            kind: AuthenticatorKind::Jwt,
            result: jwt_result,
            calls: calls.clone(),
        };
        let access_token = RecordingAuthenticator {
            kind: AuthenticatorKind::AccessToken,
            result: access_result,
            calls: calls.clone(),
        };
        (ApiAuthService::new(jwt, access_token), calls)
    }

    #[tokio::test]
    async fn should_route_jwt_shaped_token_to_cognito_authenticator() {
        let user_id = UserId::new();
        let (service, calls) = service(
            Ok(TransportPrincipal::User {
                user_id,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            }),
            Err(StaticAuthError::InvalidCredentials),
        );
        let metadata = metadata();

        let result = service
            .authenticate("header.claims.signature", &metadata)
            .await;

        let calls = lock(&calls).clone();
        assert!(
            matches!(result, Ok(TransportPrincipal::User { user_id: actual, .. }) if actual == user_id)
        );
        assert_eq!(1, calls.len());
        assert_eq!(AuthenticatorKind::Jwt, calls[0].0);
        assert_eq!("header.claims.signature", calls[0].1);
        assert_eq!(metadata, calls[0].2);
    }

    #[tokio::test]
    async fn should_route_aura_access_token_to_access_token_authenticator() {
        let user_id = UserId::new();
        let (service, calls) = service(
            Err(StaticAuthError::InvalidCredentials),
            Ok(TransportPrincipal::User {
                user_id,
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
            }),
        );

        let result = service
            .authenticate("aurahistoria_accesstoken_short_long", &metadata())
            .await;

        let calls = lock(&calls).clone();
        assert!(
            matches!(result, Ok(TransportPrincipal::User { user_id: actual, .. }) if actual == user_id)
        );
        assert_eq!(1, calls.len());
        assert_eq!(AuthenticatorKind::AccessToken, calls[0].0);
    }

    #[tokio::test]
    async fn should_reject_unknown_token_shape() {
        let (service, calls) = service(
            Err(StaticAuthError::InvalidCredentials),
            Err(StaticAuthError::InvalidCredentials),
        );

        let result = service
            .authenticate("not-a-supported-token", &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        assert!(lock(&calls).is_empty());
    }

    #[tokio::test]
    async fn should_propagate_selected_authenticator_error() {
        let (service, calls) = service(
            Err(StaticAuthError::InvalidCredentials),
            Ok(TransportPrincipal::Anonymous),
        );

        let result = service
            .authenticate("header.claims.signature", &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        assert_eq!(1, lock(&calls).len());
    }
}
