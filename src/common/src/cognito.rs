use crate::{
    api::{
        error::ApiError,
        error_code::{BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR, INVALID_UUID},
    },
    user_id::UserId,
};
use jsonwebtokens::Verifier;
use jsonwebtokens_cognito::KeySet;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum VerifyExtractCognitoJwtUserIdError {
    #[error("JwtCognitoError: {0}")]
    JwtCognito(#[from] jsonwebtokens_cognito::Error),

    #[error("JwtError: {0}")]
    JwtError(#[from] jsonwebtokens::error::Error),

    #[error("ClaimIsNotString: '{0}'")]
    ClaimIsNotString(&'static str),

    #[error("MissingClaim: '{0}'")]
    MissingClaim(&'static str),

    #[error("InvalidUuid for claim '{0}': '{1}'")]
    InvalidUuid(&'static str, uuid::Error),
}

impl From<VerifyExtractCognitoJwtUserIdError> for ApiError {
    fn from(value: VerifyExtractCognitoJwtUserIdError) -> Self {
        match value {
            VerifyExtractCognitoJwtUserIdError::JwtCognito(err) => {
                tracing::error!(eror = %err, "Failed extracting UserId from Access-Token.");
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
            }
            VerifyExtractCognitoJwtUserIdError::JwtError(err) => {
                tracing::error!(eror = %err, "Failed extracting UserId from Access-Token.");
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
            }
            err @ VerifyExtractCognitoJwtUserIdError::ClaimIsNotString(claim) => {
                tracing::error!(eror = %err, claim = claim, "Failed extracting UserId from Access-Token.");
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
            }
            err @ VerifyExtractCognitoJwtUserIdError::MissingClaim(claim) => {
                tracing::error!(eror = %err, claim = claim, "Failed extracting UserId from Access-Token.");
                ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Authorization")
            }
            VerifyExtractCognitoJwtUserIdError::InvalidUuid(claim, err) => {
                tracing::error!(eror = %err, claim = claim, "Failed extracting UserId from Access-Token.");
                ApiError::bad_request(INVALID_UUID)
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait VerifyExtractCognitoJwtUserId {
    async fn verify_extract_user_id_from_access_token(
        &self,
        authorization_token: &str,
    ) -> Result<UserId, VerifyExtractCognitoJwtUserIdError>;
}

#[derive(Clone)]
pub struct VerifyExtractCognitoJwtUserIdImpl<'a> {
    pub region: &'a str,
    pub user_pool_id: &'a str,
    pub client_ids: &'a [&'a str],
    pub keyset: KeySet,
    pub verifier: Verifier,
}

impl<'a> VerifyExtractCognitoJwtUserIdImpl<'a> {
    pub fn new(
        region: &'a str,
        user_pool_id: &'a str,
        client_ids: &'a [&'a str],
    ) -> Result<Self, VerifyExtractCognitoJwtUserIdError> {
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
impl<'a> VerifyExtractCognitoJwtUserId for VerifyExtractCognitoJwtUserIdImpl<'a> {
    async fn verify_extract_user_id_from_access_token(
        &self,
        authorization_token: &str,
    ) -> Result<UserId, VerifyExtractCognitoJwtUserIdError> {
        let claims_value: Value = self
            .keyset
            .verify(authorization_token, &self.verifier)
            .await?;

        let user_id = claims_value
            .get("sub")
            .map(|sub_val| match sub_val.as_str() {
                Some(sub) => Ok(sub),
                None => Err(VerifyExtractCognitoJwtUserIdError::ClaimIsNotString("sub")),
            })
            .ok_or(VerifyExtractCognitoJwtUserIdError::MissingClaim("sub"))?
            .map(UserId::try_from)?
            .map_err(|err| VerifyExtractCognitoJwtUserIdError::InvalidUuid("sub", err))?;

        Ok(user_id)
    }
}
