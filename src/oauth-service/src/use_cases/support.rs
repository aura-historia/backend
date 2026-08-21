use crate::error::OAuthServiceError;
use crate::ports::OAuthClientRepository;
use application::operation_context::{CredentialCapability, OperationContext};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{OAuthCodeChallenge, OAuthCodeVerifier};
use oauth_core::client::OAuthClient;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use user_core::access_token::RawOAuthClientSecret;

pub(crate) const AUTHORIZATION_CODE_TTL: time::Duration = time::Duration::minutes(10);
pub(crate) const THIRD_PARTY_EXCHANGE_CODE_TTL: time::Duration = time::Duration::seconds(60);

pub(crate) fn authorize_oauth_admin(context: &OperationContext) -> Result<(), OAuthServiceError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .authorize::<OAuthServiceError>()
}

pub(crate) async fn find_client<R: OAuthClientRepository>(
    reader: &R,
    client_id: &OAuthClientId,
) -> Result<OAuthClient, OAuthServiceError> {
    reader
        .find_by_client_id(client_id)
        .await?
        .ok_or(OAuthServiceError::ClientNotFound)
}

pub(crate) async fn authenticate_client<R: OAuthClientRepository>(
    reader: &R,
    client_id: &OAuthClientId,
    client_secret: &RawOAuthClientSecret,
) -> Result<OAuthClient, OAuthServiceError> {
    let client = find_client(reader, client_id).await?;
    if client_secret.check(&client.hashed_client_secret) {
        Ok(client)
    } else {
        Err(OAuthServiceError::InvalidClientSecret)
    }
}

pub(crate) fn append_query_params(uri: &url::Url, params: HashMap<&str, String>) -> String {
    let mut url = uri.clone();
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, &value);
    }
    url.to_string()
}

pub(crate) fn verify_s256(
    verifier: &OAuthCodeVerifier,
    expected_challenge: &OAuthCodeChallenge,
) -> bool {
    let digest = Sha256::digest(verifier.as_ref().as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == expected_challenge.as_ref()
}

pub(crate) fn validate_redirect_uris(redirect_uris: &HashSet<url::Url>) -> Result<(), String> {
    if redirect_uris.is_empty() {
        return Err("redirect_uris cannot be empty".to_owned());
    }
    for uri in redirect_uris {
        if uri.scheme() != "https" {
            return Err(format!("redirect_uri must use https: {uri}"));
        }
        if uri.fragment().is_some() {
            return Err(format!("redirect_uri must not contain fragment: {uri}"));
        }
    }
    Ok(())
}
