use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use common::{
    api::{
        error::ApiError,
        error_code::{BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR, INVALID_UUID},
    },
    user_id::UserId,
};
use http::{
    HeaderMap,
    header::{AUTHORIZATION, ToStrError},
};
use jsonwebtokens::{Algorithm, AlgorithmID, Verifier};
use jsonwebtokens_cognito::KeySet;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct JwkRsaKey {
    kid: String,
    alg: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<JwkRsaKey>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenVerifierError {
    #[error("HttpHeaderValueToStrError: {0}")]
    HttpHeaderValueToStrError(#[from] ToStrError),

    #[error("JwtAccessTokenVerifierError: {0}")]
    JwtCognito(#[from] jsonwebtokens_cognito::Error),

    #[error("JwtError: {0}")]
    JwtError(#[from] jsonwebtokens::error::Error),

    #[error("JwksFetchError: {0}")]
    JwksFetchError(String),

    #[error("ClaimIsNotString: '{0}'")]
    ClaimIsNotString(&'static str),

    #[error("MissingClaim: '{0}'")]
    MissingClaim(&'static str),

    #[error("InvalidUuid for claim '{0}': '{1}'")]
    InvalidUuid(&'static str, uuid::Error),
}

impl From<AccessTokenVerifierError> for ApiError {
    fn from(value: AccessTokenVerifierError) -> Self {
        match value {
            AccessTokenVerifierError::HttpHeaderValueToStrError(ref err) => {
                let msg = err.to_string();
                ApiError::bad_request(BAD_HEADER_VALUE, Box::new(value))
                    .with_header_field(AUTHORIZATION.as_str())
                    .with_detail(msg)
            }
            AccessTokenVerifierError::JwtCognito(_) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
            }
            AccessTokenVerifierError::JwtError(_) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
            }
            AccessTokenVerifierError::JwksFetchError(_) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
            }
            AccessTokenVerifierError::ClaimIsNotString(_) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
            }
            AccessTokenVerifierError::MissingClaim(claim) => {
                ApiError::bad_request(BAD_HEADER_VALUE, Box::new(value))
                    .with_header_field(AUTHORIZATION.as_str())
                    .with_detail(format!("Missing claim '{claim}'."))
            }
            AccessTokenVerifierError::InvalidUuid(claim, _) => {
                ApiError::bad_request(INVALID_UUID, Box::new(value)).with_detail(format!(
                    "String-Value for decoded claim '{claim}' is not a valid UUID."
                ))
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait AccessTokenVerifierService {
    async fn verify_extract_user_id(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<UserId>, AccessTokenVerifierError> {
        match extract_access_token(headers) {
            Ok(Some(access_token)) => self
                .verify_extract_user_id_from_access_token(&access_token)
                .await
                .map(Some),
            Ok(None) => Ok(None),
            Err(err) => Err(AccessTokenVerifierError::HttpHeaderValueToStrError(err)),
        }
    }

    async fn verify_extract_user_id_from_access_token(
        &self,
        access_token: &str,
    ) -> Result<UserId, AccessTokenVerifierError>;
}

#[derive(Clone)]
pub struct AccessTokenVerifierServiceImpl<'a> {
    pub region: &'a str,
    pub user_pool_id: &'a str,
    pub client_ids: &'a [&'a str],
    pub keyset: KeySet,
    pub verifier: Verifier,
    custom_jwks_url: Option<String>,
    jwks_cache: Arc<RwLock<HashMap<String, Arc<Algorithm>>>>,
}

impl<'a> AccessTokenVerifierServiceImpl<'a> {
    pub fn new(
        region: &'a str,
        user_pool_id: &'a str,
        client_ids: &'a [&'a str],
    ) -> Result<Self, AccessTokenVerifierError> {
        let keyset = KeySet::new(region, user_pool_id)?;
        let verifier = keyset.new_access_token_verifier(client_ids).build()?;
        Ok(Self {
            region,
            user_pool_id,
            client_ids,
            keyset,
            verifier,
            custom_jwks_url: None,
            jwks_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Creates an instance that fetches JWKS from a custom endpoint instead of
    /// the standard AWS Cognito endpoint.
    ///
    /// Inside a LocalStack Lambda container the real
    /// `https://cognito-idp.{region}.amazonaws.com` is unreachable.
    /// Pass the LocalStack-reachable base URL (e.g.
    /// `http://host.docker.internal:{port}`) so that the JWKS can be fetched
    /// from `{cognito_idp_endpoint}/{user_pool_id}/.well-known/jwks.json`.
    pub fn new_with_cognito_idp_endpoint(
        cognito_idp_endpoint: &str,
        region: &'a str,
        user_pool_id: &'a str,
        client_ids: &'a [&'a str],
    ) -> Result<Self, AccessTokenVerifierError> {
        let jwks_url = format!(
            "{}/{}/.well-known/jwks.json",
            cognito_idp_endpoint.trim_end_matches('/'),
            user_pool_id
        );
        let keyset = KeySet::new(region, user_pool_id)?;
        let verifier = keyset.new_access_token_verifier(client_ids).build()?;
        Ok(Self {
            region,
            user_pool_id,
            client_ids,
            keyset,
            verifier,
            custom_jwks_url: Some(jwks_url),
            jwks_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn fetch_and_cache_jwks(&self, jwks_url: &str) -> Result<(), AccessTokenVerifierError> {
        let resp = reqwest::get(jwks_url)
            .await
            .map_err(|e| AccessTokenVerifierError::JwksFetchError(e.to_string()))?;
        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|e| AccessTokenVerifierError::JwksFetchError(e.to_string()))?;

        let mut cache = self.jwks_cache.write().unwrap();
        for key in jwks.keys {
            if key.alg != "RS256" {
                continue;
            }
            let mut algorithm =
                Algorithm::new_rsa_n_e_b64_verifier(AlgorithmID::RS256, &key.n, &key.e)?;
            algorithm.set_kid(&key.kid);
            cache.insert(key.kid, Arc::new(algorithm));
        }

        Ok(())
    }

    async fn verify_with_custom_jwks(
        &self,
        access_token: &str,
        jwks_url: &str,
    ) -> Result<Value, AccessTokenVerifierError> {
        let header = jsonwebtokens::raw::decode_header_only(access_token)?;
        let kid = header
            .get("kid")
            .and_then(|v| v.as_str())
            .ok_or(AccessTokenVerifierError::MissingClaim("kid"))?;

        // Try cache first.
        {
            let cache = self.jwks_cache.read().unwrap();
            if let Some(alg) = cache.get(kid) {
                return Ok(self.verifier.verify(access_token, alg)?);
            }
        }

        // Cache miss — fetch from remote JWKS endpoint and retry.
        self.fetch_and_cache_jwks(jwks_url).await?;

        let cache = self.jwks_cache.read().unwrap();
        let alg = cache
            .get(kid)
            .ok_or(AccessTokenVerifierError::MissingClaim("kid"))?;
        Ok(self.verifier.verify(access_token, alg)?)
    }
}

#[async_trait::async_trait]
impl<'a> AccessTokenVerifierService for AccessTokenVerifierServiceImpl<'a> {
    async fn verify_extract_user_id_from_access_token(
        &self,
        access_token: &str,
    ) -> Result<UserId, AccessTokenVerifierError> {
        let claims_value: Value = match &self.custom_jwks_url {
            Some(jwks_url) => self.verify_with_custom_jwks(access_token, jwks_url).await?,
            None => self.keyset.verify(access_token, &self.verifier).await?,
        };

        let user_id = claims_value
            .get("sub")
            .map(|sub_val| match sub_val.as_str() {
                Some(sub) => Ok(sub),
                None => Err(AccessTokenVerifierError::ClaimIsNotString("sub")),
            })
            .ok_or(AccessTokenVerifierError::MissingClaim("sub"))?
            .map(UserId::try_from)?
            .map_err(|err| AccessTokenVerifierError::InvalidUuid("sub", err))?;

        Ok(user_id)
    }
}

pub fn extract_access_token(headers: &HeaderMap) -> Result<Option<String>, ToStrError> {
    let access_token = headers
        .get(AUTHORIZATION)
        .map(|v| v.to_str())
        .transpose()?
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).to_owned());
    Ok(access_token)
}
