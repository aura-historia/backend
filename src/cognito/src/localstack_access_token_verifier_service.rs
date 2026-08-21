use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use jsonwebtokens::{Algorithm, AlgorithmID, Verifier};
use serde::Deserialize;
use serde_json::Value;
use user_core::user_id::UserId;

use crate::access_token_verifier_service::{
    AccessTokenVerifierError, AccessTokenVerifierService, extract_sub_claim,
};

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

/// [`AccessTokenVerifierService`] implementation that fetches JWKS from a
/// custom endpoint instead of the standard AWS Cognito endpoint.
///
/// Inside a LocalStack Lambda container the real
/// `https://cognito-idp.{region}.amazonaws.com` is unreachable.
///
/// Two separate base URLs are required because they serve different purposes:
///
/// * `cognito_idp_endpoint` — reachable from inside the Lambda container
///   (e.g. `http://host.docker.internal:{mapped_port}`). Used only to fetch
///   `{cognito_idp_endpoint}/{user_pool_id}/.well-known/jwks.json`.
///
/// * `cognito_issuer_base_url` — the URL that LocalStack embeds in the `iss`
///   claim of every token it mints (e.g. `http://localhost.localstack.cloud:4566`).
///   Used only for JWT `iss` claim verification.
#[derive(Clone)]
pub struct LocalStackAccessTokenVerifierServiceImpl {
    jwks_url: String,
    verifier: Verifier,
    jwks_cache: Arc<RwLock<HashMap<String, Arc<Algorithm>>>>,
}

impl LocalStackAccessTokenVerifierServiceImpl {
    pub fn new(
        cognito_idp_endpoint: &str,
        cognito_issuer_base_url: &str,
        user_pool_id: &str,
        client_ids: &[&str],
    ) -> Result<Self, AccessTokenVerifierError> {
        let cognito_idp_endpoint = cognito_idp_endpoint.trim_end_matches('/');
        let jwks_url = format!(
            "{}/{}/.well-known/jwks.json",
            cognito_idp_endpoint, user_pool_id
        );
        // LocalStack issues tokens with iss = "{issuer_base}/{pool_id}" rather than
        // the standard "https://cognito-idp.{region}.amazonaws.com/{pool_id}".
        // Build the Verifier directly so we can supply the correct issuer URL.
        let issuer = format!(
            "{}/{}",
            cognito_issuer_base_url.trim_end_matches('/'),
            user_pool_id
        );
        let verifier = Verifier::create()
            .issuer(&issuer)
            .string_equals_one_of("client_id", client_ids)
            .string_equals("token_use", "access")
            .build()?;
        Ok(Self {
            jwks_url,
            verifier,
            jwks_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn fetch_and_cache_jwks(&self) -> Result<(), AccessTokenVerifierError> {
        let resp = reqwest::get(&self.jwks_url)
            .await
            .map_err(|e| AccessTokenVerifierError::JwksFetchError(e.to_string()))?;
        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|e| AccessTokenVerifierError::JwksFetchError(e.to_string()))?;

        let mut cache = self
            .jwks_cache
            .write()
            .expect("JWKS cache lock should not be poisoned");
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
}

#[async_trait::async_trait]
impl AccessTokenVerifierService for LocalStackAccessTokenVerifierServiceImpl {
    async fn verify_extract_user_id_from_access_token(
        &self,
        access_token: &str,
    ) -> Result<UserId, AccessTokenVerifierError> {
        let header = jsonwebtokens::raw::decode_header_only(access_token)?;
        let kid = header
            .get("kid")
            .and_then(|v| v.as_str())
            .ok_or(AccessTokenVerifierError::MissingClaim("kid"))?;

        // Try cache first.
        {
            let cache = self
                .jwks_cache
                .read()
                .expect("JWKS cache lock should not be poisoned");
            if let Some(alg) = cache.get(kid) {
                let claims: Value = self.verifier.verify(access_token, alg)?;
                return extract_sub_claim(&claims);
            }
        }

        // Cache miss — fetch from remote JWKS endpoint and retry.
        self.fetch_and_cache_jwks().await?;

        let cache = self
            .jwks_cache
            .read()
            .expect("JWKS cache lock should not be poisoned");
        let alg = cache
            .get(kid)
            .ok_or(AccessTokenVerifierError::MissingClaim("kid"))?;
        let claims: Value = self.verifier.verify(access_token, alg)?;
        extract_sub_claim(&claims)
    }
}
