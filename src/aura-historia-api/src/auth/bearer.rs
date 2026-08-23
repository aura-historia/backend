use crate::auth::core::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
use http::{HeaderMap, header::AUTHORIZATION};

pub struct OptionalAuthExtractor<'a, A: ?Sized> {
    authenticator: &'a A,
}

impl<'a, A: ?Sized> OptionalAuthExtractor<'a, A> {
    pub fn new(authenticator: &'a A) -> Self {
        Self { authenticator }
    }
}

impl<A> OptionalAuthExtractor<'_, A>
where
    A: TokenAuthenticator + ?Sized,
{
    pub async fn extract(
        &self,
        headers: &HeaderMap,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        match extract_bearer_token(headers)? {
            None => Ok(TransportPrincipal::Anonymous),
            Some(token) => self.authenticator.authenticate(&token, metadata).await,
        }
    }
}

pub struct ProtectedAuthExtractor<'a, A: ?Sized> {
    authenticator: &'a A,
}

impl<'a, A: ?Sized> ProtectedAuthExtractor<'a, A> {
    pub fn new(authenticator: &'a A) -> Self {
        Self { authenticator }
    }
}

impl<A> ProtectedAuthExtractor<'_, A>
where
    A: TokenAuthenticator + ?Sized,
{
    pub async fn extract(
        &self,
        headers: &HeaderMap,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let token = extract_bearer_token(headers)?.ok_or(AuthError::MissingCredentials)?;
        self.authenticator.authenticate(&token, metadata).await
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<Option<String>, AuthError> {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AuthError::InvalidAuthorizationHeader)?;
    value
        .strip_prefix("Bearer ")
        .map(|token| Some(token.to_owned()))
        .ok_or(AuthError::InvalidAuthorizationHeader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::core::AuthMethod;
    use application::operation_context::CredentialCapability;
    use http::HeaderValue;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;

    type AuthCall = (String, RequestMetadata);
    type AuthCalls = Arc<Mutex<Vec<AuthCall>>>;

    #[derive(Clone)]
    struct StaticAuthenticator {
        result: Result<TransportPrincipal, StaticAuthError>,
        calls: AuthCalls,
    }

    #[derive(Debug, Clone, Copy)]
    enum StaticAuthError {
        InvalidCredentials,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for StaticAuthenticator {
        async fn authenticate(
            &self,
            bearer_token: &str,
            metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            lock(&self.calls).push((bearer_token.to_owned(), metadata.clone()));
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
        RequestMetadata::new("server-req-1", "corr-1")
    }

    fn headers(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
        headers
    }

    fn authenticator(
        result: Result<TransportPrincipal, StaticAuthError>,
    ) -> (StaticAuthenticator, AuthCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            StaticAuthenticator {
                result,
                calls: calls.clone(),
            },
            calls,
        )
    }

    #[test]
    fn should_extract_none_when_authorization_header_missing() {
        assert!(matches!(extract_bearer_token(&HeaderMap::new()), Ok(None)));
    }

    #[test]
    fn should_extract_token_when_authorization_header_is_bearer() {
        assert!(matches!(
            extract_bearer_token(&headers("Bearer token-1")),
            Ok(Some(token)) if token == "token-1"
        ));
    }

    #[test]
    fn should_reject_token_when_authorization_header_not_bearer() {
        assert!(matches!(
            extract_bearer_token(&headers("Basic token-1")),
            Err(AuthError::InvalidAuthorizationHeader)
        ));
    }

    #[test]
    fn should_reject_token_when_authorization_header_not_ascii() -> Result<(), http::Error> {
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_bytes(b"Bearer \xff")?;
        headers.insert(AUTHORIZATION, value);

        assert!(matches!(
            extract_bearer_token(&headers),
            Err(AuthError::InvalidAuthorizationHeader)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_allow_optional_auth_when_header_missing() {
        let (authenticator, calls) = authenticator(Ok(TransportPrincipal::Anonymous));
        let extractor = OptionalAuthExtractor::new(&authenticator);

        let principal = extractor.extract(&HeaderMap::new(), &metadata()).await;

        assert!(matches!(principal, Ok(TransportPrincipal::Anonymous)));
        assert!(lock(&calls).is_empty());
    }

    #[tokio::test]
    async fn should_accept_optional_auth_when_supplied_token_valid() {
        let user_id = UserId::new();
        let (authenticator, calls) = authenticator(Ok(TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::CognitoJwt,
            capabilities: BTreeSet::new(),
        }));
        let extractor = OptionalAuthExtractor::new(&authenticator);
        let metadata = metadata();

        let principal = extractor
            .extract(&headers("Bearer good-token"), &metadata)
            .await;

        let calls = lock(&calls).clone();
        assert!(
            matches!(principal, Ok(TransportPrincipal::User { user_id: actual, .. }) if actual == user_id)
        );
        assert_eq!(1, calls.len());
        assert_eq!("good-token", calls[0].0);
        assert_eq!(metadata, calls[0].1);
    }

    #[tokio::test]
    async fn should_reject_optional_auth_when_supplied_token_invalid() {
        let (authenticator, _calls) = authenticator(Err(StaticAuthError::InvalidCredentials));
        let extractor = OptionalAuthExtractor::new(&authenticator);

        let principal = extractor
            .extract(&headers("Bearer bad-token"), &metadata())
            .await;

        assert!(matches!(principal, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn should_reject_protected_auth_when_header_missing() {
        let (authenticator, calls) = authenticator(Ok(TransportPrincipal::Anonymous));
        let extractor = ProtectedAuthExtractor::new(&authenticator);

        let principal = extractor.extract(&HeaderMap::new(), &metadata()).await;

        assert!(matches!(principal, Err(AuthError::MissingCredentials)));
        assert!(lock(&calls).is_empty());
    }

    #[tokio::test]
    async fn should_accept_protected_auth_when_token_valid() {
        let user_id = UserId::new();
        let (authenticator, calls) = authenticator(Ok(TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
        }));
        let extractor = ProtectedAuthExtractor::new(&authenticator);

        let principal = extractor
            .extract(&headers("Bearer good-token"), &metadata())
            .await;

        assert!(
            matches!(principal, Ok(TransportPrincipal::User { user_id: actual, .. }) if actual == user_id)
        );
        assert_eq!(1, lock(&calls).len());
    }
}
