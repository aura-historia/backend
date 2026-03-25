use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use common::user_id::UserId;
use jsonwebtokens::{Algorithm, AlgorithmID, Verifier};
use jsonwebtokens_cognito::KeySet;
use serde::Deserialize;
use serde_json::Value;

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
/// This implementation accepts an arbitrary Cognito IDP base URL
/// (e.g. `http://host.docker.internal:{port}`) and fetches the JWKS from
/// `{cognito_idp_endpoint}/{user_pool_id}/.well-known/jwks.json`.
#[derive(Clone)]
pub struct LocalStackAccessTokenVerifierServiceImpl {
    jwks_url: String,
    verifier: Verifier,
    jwks_cache: Arc<RwLock<HashMap<String, Arc<Algorithm>>>>,
}

impl LocalStackAccessTokenVerifierServiceImpl {
    pub fn new(
        cognito_idp_endpoint: &str,
        region: &str,
        user_pool_id: &str,
        client_ids: &[&str],
    ) -> Result<Self, AccessTokenVerifierError> {
        let jwks_url = format!(
            "{}/{}/.well-known/jwks.json",
            cognito_idp_endpoint.trim_end_matches('/'),
            user_pool_id
        );
        let keyset = KeySet::new(region, user_pool_id)?;
        let verifier = keyset.new_access_token_verifier(client_ids).build()?;
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
