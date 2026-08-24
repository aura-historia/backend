use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientAuthenticationReader, OAuthClientRepository};
use application::operation_context::{CredentialCapability, OperationContext};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{OAuthCodeChallenge, OAuthCodeVerifier};
use oauth_core::client::OAuthClient;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use user_core::access_token::RawOAuthClientSecret;

pub(crate) const AUTHORIZATION_CODE_TTL: time::Duration = time::Duration::minutes(10);
pub(crate) const THIRD_PARTY_EXCHANGE_CODE_TTL: time::Duration = time::Duration::seconds(60);

pub(crate) fn authorize_oauth_admin(context: &OperationContext) -> Result<(), OAuthServiceError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .authorize::<OAuthServiceError>()
}

pub(crate) fn authorize_oauth_client_read(
    context: &OperationContext,
) -> Result<(), OAuthServiceError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensRead)
        .authorize::<OAuthServiceError>()
}

pub(crate) async fn authenticate_client<R: OAuthClientRepository>(
    repository: &mut R,
    client_id: &OAuthClientId,
    client_secret: &RawOAuthClientSecret,
) -> Result<OAuthClient, OAuthServiceError> {
    let client = repository
        .find_by_id(*client_id)
        .await?
        .ok_or(OAuthServiceError::ClientNotFound)?
        .value;
    if client_secret.check(client.hashed_client_secret()) {
        Ok(client)
    } else {
        Err(OAuthServiceError::InvalidClientSecret)
    }
}

pub(crate) async fn authenticate_client_reader<R: OAuthClientAuthenticationReader>(
    reader: &R,
    client_id: &OAuthClientId,
    client_secret: &RawOAuthClientSecret,
) -> Result<(), OAuthServiceError> {
    let client = reader
        .find_by_id(client_id)
        .await?
        .ok_or(OAuthServiceError::ClientNotFound)?;
    if client_secret.check(&client.hashed_client_secret) {
        Ok(())
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
