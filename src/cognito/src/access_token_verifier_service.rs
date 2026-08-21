use http::{
    HeaderMap,
    header::{AUTHORIZATION, ToStrError},
};
use jsonwebtokens::Verifier;
use jsonwebtokens_cognito::KeySet;
use serde_json::Value;
use user_core::user_id::UserId;

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
}

impl<'a> AccessTokenVerifierServiceImpl<'a> {
    pub fn new(
        region: &'a str,
        user_pool_id: &'a str,
        client_ids: &'a [&'a str],
    ) -> Result<Self, AccessTokenVerifierError> {
        let keyset = KeySet::new(region, user_pool_id)?;
        let verifier = keyset.new_access_token_verifier(client_ids).build()?;
        let val = Self {
            region,
            user_pool_id,
            client_ids,
            keyset,
            verifier,
        };
        Ok(val)
    }
}

#[async_trait::async_trait]
impl<'a> AccessTokenVerifierService for AccessTokenVerifierServiceImpl<'a> {
    async fn verify_extract_user_id_from_access_token(
        &self,
        access_token: &str,
    ) -> Result<UserId, AccessTokenVerifierError> {
        let claims_value: Value = self.keyset.verify(access_token, &self.verifier).await?;
        extract_sub_claim(&claims_value)
    }
}

pub(crate) fn extract_sub_claim(claims: &Value) -> Result<UserId, AccessTokenVerifierError> {
    claims
        .get("sub")
        .map(|sub_val| match sub_val.as_str() {
            Some(sub) => Ok(sub),
            None => Err(AccessTokenVerifierError::ClaimIsNotString("sub")),
        })
        .ok_or(AccessTokenVerifierError::MissingClaim("sub"))?
        .map(UserId::try_from)?
        .map_err(|err| AccessTokenVerifierError::InvalidUuid("sub", err))
}

#[async_trait::async_trait]
impl<T: AccessTokenVerifierService + Send + Sync + ?Sized> AccessTokenVerifierService for Box<T> {
    async fn verify_extract_user_id_from_access_token(
        &self,
        access_token: &str,
    ) -> Result<UserId, AccessTokenVerifierError> {
        (**self)
            .verify_extract_user_id_from_access_token(access_token)
            .await
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
