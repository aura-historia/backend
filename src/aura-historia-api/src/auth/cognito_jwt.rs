use crate::auth::core::{
    AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
};
use common::user_id::UserId;
use jsonwebtokens::{Algorithm, AlgorithmID, Verifier, raw};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use user_core::access_token::Scope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CognitoJwtConfig {
    pub issuer: String,
    pub jwks_url: String,
    pub audiences: HashSet<String>,
}

impl CognitoJwtConfig {
    pub fn new(
        issuer: impl Into<String>,
        jwks_url: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            jwks_url: jwks_url.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonWebKeySet {
    pub keys: Vec<JsonWebKey>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonWebKey {
    pub kid: String,
    pub alg: Option<String>,
    pub n: String,
    pub e: String,
}

#[async_trait::async_trait]
pub trait JwksProvider: Send + Sync {
    async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError>;
}

#[derive(Clone)]
pub struct ReqwestJwksProvider {
    client: reqwest::Client,
}

impl ReqwestJwksProvider {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl JwksProvider for ReqwestJwksProvider {
    async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
        self.client
            .get(jwks_url)
            .send()
            .await
            .map_err(|error| AuthError::JwksFetch(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::JwksFetch(error.to_string()))?
            .json()
            .await
            .map_err(|error| AuthError::JwksFetch(error.to_string()))
    }
}

pub struct CognitoJwtAuthenticator<P> {
    config: CognitoJwtConfig,
    provider: P,
    verifier: Verifier,
    jwks_cache: Arc<RwLock<HashMap<String, Arc<Algorithm>>>>,
}

impl<P> CognitoJwtAuthenticator<P> {
    pub fn new(config: CognitoJwtConfig, provider: P) -> Result<Self, AuthError> {
        let verifier = Verifier::create()
            .issuer(config.issuer.clone())
            .build()
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        Ok(Self {
            config,
            provider,
            verifier,
            jwks_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

impl<P> CognitoJwtAuthenticator<P>
where
    P: JwksProvider,
{
    async fn algorithm_for_token(&self, token: &str) -> Result<Arc<Algorithm>, AuthError> {
        let header = raw::decode_header_only(token).map_err(|_| AuthError::MalformedCredentials)?;
        let kid = claim_string(&header, "kid")?;

        if let Some(algorithm) = self.cached_algorithm(kid)? {
            return Ok(algorithm);
        }

        self.refresh_jwks().await?;
        self.cached_algorithm(kid)?
            .ok_or(AuthError::JwksKeyNotFound)
    }

    fn cached_algorithm(&self, kid: &str) -> Result<Option<Arc<Algorithm>>, AuthError> {
        let cache = self
            .jwks_cache
            .read()
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        Ok(cache.get(kid).cloned())
    }

    async fn refresh_jwks(&self) -> Result<(), AuthError> {
        let jwks = self.provider.fetch_jwks(&self.config.jwks_url).await?;
        let mut cache = self
            .jwks_cache
            .write()
            .map_err(|error| AuthError::Internal(error.to_string()))?;

        for key in jwks.keys {
            if !matches!(key.alg.as_deref(), None | Some("RS256")) {
                continue;
            }
            let mut algorithm =
                Algorithm::new_rsa_n_e_b64_verifier(AlgorithmID::RS256, &key.n, &key.e)
                    .map_err(|error| AuthError::Internal(error.to_string()))?;
            algorithm.set_kid(&key.kid);
            cache.insert(key.kid, Arc::new(algorithm));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<P> TokenAuthenticator for CognitoJwtAuthenticator<P>
where
    P: JwksProvider,
{
    async fn authenticate(
        &self,
        bearer_token: &str,
        required_scopes: &HashSet<Scope>,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let algorithm = self.algorithm_for_token(bearer_token).await?;
        let claims = self
            .verifier
            .verify(bearer_token, &algorithm)
            .map_err(|_| AuthError::InvalidCredentials)?;

        verify_audience(&claims, &self.config.audiences)?;
        if !required_scopes.is_empty() {
            return Err(AuthError::InsufficientScope);
        }
        let user_id = UserId::try_from(claim_string(&claims, "sub")?)
            .map_err(|_| AuthError::InvalidCredentials)?;

        Ok(TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::CognitoJwt,
            scopes: HashSet::new(),
        })
    }
}

fn verify_audience(claims: &Value, audiences: &HashSet<String>) -> Result<(), AuthError> {
    let matched = claims
        .get("aud")
        .map(|aud| audience_claim_matches(aud, audiences, "aud"))
        .transpose()?
        .unwrap_or(false)
        || claims
            .get("client_id")
            .map(|client_id| audience_claim_matches(client_id, audiences, "client_id"))
            .transpose()?
            .unwrap_or(false);

    if matched {
        Ok(())
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

fn audience_claim_matches(
    value: &Value,
    audiences: &HashSet<String>,
    claim: &'static str,
) -> Result<bool, AuthError> {
    match value {
        Value::String(audience) => Ok(audiences.contains(audience)),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::String(audience) => Ok(audiences.contains(audience)),
                _ => Err(AuthError::InvalidClaimType(claim)),
            })
            .try_fold(false, |matched, value| value.map(|value| matched || value)),
        _ => Err(AuthError::InvalidClaimType(claim)),
    }
}

fn claim_string<'a>(claims: &'a Value, claim: &'static str) -> Result<&'a str, AuthError> {
    claims
        .get(claim)
        .ok_or(AuthError::MissingClaim(claim))?
        .as_str()
        .ok_or(AuthError::InvalidClaimType(claim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtokens as jwt;
    use openssl::rsa::Rsa;
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard};
    use time::OffsetDateTime;

    #[derive(Clone)]
    struct FakeJwksProvider {
        result: Result<JsonWebKeySet, FakeJwksError>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Debug, Clone, Copy)]
    enum FakeJwksError {
        Fetch,
    }

    #[async_trait::async_trait]
    impl JwksProvider for FakeJwksProvider {
        async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
            lock(&self.calls).push(jwks_url.to_owned());
            self.result
                .clone()
                .map_err(|_| AuthError::JwksFetch("boom".to_owned()))
        }
    }

    #[derive(Clone)]
    struct TestKey {
        kid: String,
        private_pem: Vec<u8>,
        n: String,
        e: String,
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

    fn test_key(kid: &str) -> Result<TestKey, Box<dyn std::error::Error>> {
        let rsa = Rsa::generate(2048)?;
        let private_pem = rsa.private_key_to_pem()?;
        Ok(TestKey {
            kid: kid.to_owned(),
            private_pem,
            n: URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
            e: URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
        })
    }

    fn jwk(key: &TestKey) -> JsonWebKey {
        JsonWebKey {
            kid: key.kid.clone(),
            alg: Some("RS256".to_owned()),
            n: key.n.clone(),
            e: key.e.clone(),
        }
    }

    fn authenticator(
        keys: Vec<JsonWebKey>,
        calls: Arc<Mutex<Vec<String>>>,
    ) -> Result<CognitoJwtAuthenticator<FakeJwksProvider>, AuthError> {
        authenticator_with_provider_result(Ok(JsonWebKeySet { keys }), calls)
    }

    fn authenticator_with_provider_result(
        result: Result<JsonWebKeySet, FakeJwksError>,
        calls: Arc<Mutex<Vec<String>>>,
    ) -> Result<CognitoJwtAuthenticator<FakeJwksProvider>, AuthError> {
        CognitoJwtAuthenticator::new(
            CognitoJwtConfig::new(
                "https://issuer.example/pool",
                "https://issuer.example/pool/.well-known/jwks.json",
                ["audience-1"],
            ),
            FakeJwksProvider { result, calls },
        )
    }

    fn signed_jwt(key: &TestKey, claims: Value) -> Result<String, Box<dyn std::error::Error>> {
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name(), "kid": key.kid });
        Ok(jwt::encode(&header, &claims, &algorithm)?)
    }

    fn jwt_claims(user_id: UserId, exp_delta_seconds: i64, audience: Value) -> Value {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        json!({
            "iss": "https://issuer.example/pool",
            "sub": user_id.to_string(),
            "aud": audience,
            "iat": now,
            "exp": now + exp_delta_seconds,
        })
    }

    #[test]
    fn should_build_cognito_jwt_config() {
        let config = CognitoJwtConfig::new("issuer", "jwks", ["aud-1", "aud-2"]);

        assert_eq!("issuer", config.issuer);
        assert_eq!("jwks", config.jwks_url);
        assert_eq!(
            HashSet::from(["aud-1".to_owned(), "aud-2".to_owned()]),
            config.audiences
        );
    }

    #[test]
    fn should_create_reqwest_jwks_provider() {
        let _provider = ReqwestJwksProvider::new(reqwest::Client::new());
    }

    #[test]
    fn should_deserialize_jwks() -> Result<(), Box<dyn std::error::Error>> {
        let jwks: JsonWebKeySet = serde_json::from_value(json!({
            "keys": [{ "kid": "kid-1", "alg": "RS256", "n": "n", "e": "e" }]
        }))?;

        assert_eq!(1, jwks.keys.len());
        assert_eq!("kid-1", jwks.keys[0].kid);
        Ok(())
    }

    #[tokio::test]
    async fn should_authenticate_cognito_jwt_when_signature_claims_and_audience_valid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = authenticator(vec![jwk(&key)], calls.clone())?;
        let user_id = UserId::new();
        let token = signed_jwt(&key, jwt_claims(user_id, 3_600, json!("audience-1")))?;

        let principal = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await?;

        assert!(matches!(
            principal,
            TransportPrincipal::User {
                user_id: actual,
                auth_method: AuthMethod::CognitoJwt,
                scopes,
            } if actual == user_id && scopes.is_empty()
        ));
        assert_eq!(
            vec!["https://issuer.example/pool/.well-known/jwks.json".to_owned()],
            *lock(&calls)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_accept_cognito_jwt_when_client_id_matches_audience()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let user_id = UserId::new();
        let mut claims = jwt_claims(user_id, 3_600, json!("ignored-audience"));
        if let Some(claims) = claims.as_object_mut() {
            claims.remove("aud");
            claims.insert("client_id".to_owned(), json!("audience-1"));
        }
        let token = signed_jwt(&key, claims)?;

        let principal = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await?;

        assert!(
            matches!(principal, TransportPrincipal::User { user_id: actual, .. } if actual == user_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_accept_cognito_jwt_when_audience_array_contains_configured_audience()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let user_id = UserId::new();
        let token = signed_jwt(
            &key,
            jwt_claims(user_id, 3_600, json!(["audience-0", "audience-1"])),
        )?;

        let principal = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await?;

        assert!(
            matches!(principal, TransportPrincipal::User { user_id: actual, .. } if actual == user_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reuse_cached_jwks_when_kid_known() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = authenticator(vec![jwk(&key)], calls.clone())?;
        let user_id = UserId::new();
        let token = signed_jwt(&key, jwt_claims(user_id, 3_600, json!("audience-1")))?;

        let _ = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await?;
        let _ = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await?;

        assert_eq!(1, lock(&calls).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_route_requires_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!("audience-1")))?;

        let result = authenticator
            .authenticate(&token, &HashSet::from([Scope::ProductsWrite]), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InsufficientScope)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_expired() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), -60, json!("audience-1")))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_issuer_wrong() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(UserId::new(), 3_600, json!("audience-1"));
        claims["iss"] = json!("https://evil.example/pool");
        let token = signed_jwt(&key, claims)?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_audience_wrong()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(
            &key,
            jwt_claims(UserId::new(), 3_600, json!("other-audience")),
        )?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_audience_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!(123)))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_audience_array_value_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!([123])))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[test]
    fn should_reject_audience_claim_when_type_invalid() {
        let result = audience_claim_matches(&json!(123), &HashSet::new(), "aud");

        assert!(matches!(result, Err(AuthError::InvalidClaimType("aud"))));
    }

    #[test]
    fn should_reject_audience_claim_when_array_value_invalid() {
        let result = audience_claim_matches(&json!([123]), &HashSet::new(), "aud");

        assert!(matches!(result, Err(AuthError::InvalidClaimType("aud"))));
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_sub_missing() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(UserId::new(), 3_600, json!("audience-1"));
        if let Some(claims) = claims.as_object_mut() {
            claims.remove("sub");
        }
        let token = signed_jwt(&key, claims)?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MissingClaim("sub"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_sub_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(UserId::new(), 3_600, json!("audience-1"));
        claims["sub"] = json!(123);
        let token = signed_jwt(&key, claims)?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_sub_not_uuid() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(UserId::new(), 3_600, json!("audience-1"));
        claims["sub"] = json!("not-a-uuid");
        let token = signed_jwt(&key, claims)?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_token_malformed() -> Result<(), AuthError> {
        let authenticator = authenticator(Vec::new(), Arc::new(Mutex::new(Vec::new())))?;

        let result = authenticator
            .authenticate("not-a-jwt", &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_kid_missing() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name() });
        let token = jwt::encode(
            &header,
            &jwt_claims(UserId::new(), 3_600, json!("audience-1")),
            &algorithm,
        )?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MissingClaim("kid"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_kid_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name(), "kid": 123 });
        let token = jwt::encode(
            &header,
            &jwt_claims(UserId::new(), 3_600, json!("audience-1")),
            &algorithm,
        )?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::InvalidClaimType("kid"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_jwks_fetch_fails() -> Result<(), AuthError> {
        let authenticator = authenticator_with_provider_result(
            Err(FakeJwksError::Fetch),
            Arc::new(Mutex::new(Vec::new())),
        )?;

        let result = authenticator
            .authenticate("header.claims.signature", &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_jwks_missing_kid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let other_key = test_key("kid-2")?;
        let authenticator = authenticator(vec![jwk(&other_key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!("audience-1")))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::JwksKeyNotFound)));
        Ok(())
    }

    #[tokio::test]
    async fn should_ignore_jwks_key_when_alg_not_rs256() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let mut key_record = jwk(&key);
        key_record.alg = Some("RS384".to_owned());
        let authenticator = authenticator(vec![key_record], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!("audience-1")))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::JwksKeyNotFound)));
        Ok(())
    }

    #[tokio::test]
    async fn should_map_jwks_provider_failure_when_header_valid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator_with_provider_result(
            Err(FakeJwksError::Fetch),
            Arc::new(Mutex::new(Vec::new())),
        )?;
        let token = signed_jwt(&key, jwt_claims(UserId::new(), 3_600, json!("audience-1")))?;

        let result = authenticator
            .authenticate(&token, &HashSet::new(), &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::JwksFetch(_))));
        Ok(())
    }
}
