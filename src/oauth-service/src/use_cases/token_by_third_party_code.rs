use crate::error::OAuthServiceError;
use crate::ports::ThirdPartyExchangeCodeRepository;
use crate::use_cases::token_by_authorization_code::{OAuthTokenType, TokenResponse};
use oauth_core::third_party_exchange_code::ThirdPartyExchangeCode;

#[async_trait::async_trait]
pub trait TokenByThirdPartyCodeUseCase: Send + Sync {
    async fn execute(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

pub struct TokenByThirdPartyCodeHandler<R> {
    repository: R,
}
impl<R> TokenByThirdPartyCodeHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}
#[async_trait::async_trait]
impl<R> TokenByThirdPartyCodeUseCase for TokenByThirdPartyCodeHandler<R>
where
    R: ThirdPartyExchangeCodeRepository,
{
    async fn execute(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let grant = self
            .repository
            .find_by_code(code)
            .await?
            .ok_or(OAuthServiceError::ThirdPartyExchangeCodeNotFound)?;
        self.repository.delete(code).await?;
        if grant.is_expired() {
            return Err(OAuthServiceError::ThirdPartyExchangeCodeExpired);
        }
        Ok(TokenResponse {
            access_token: grant.access_token,
            token_type: OAuthTokenType::Bearer,
            expires: grant.access_token_expires,
            scopes: grant.scopes,
            third_party_exchange_code: None,
        })
    }
}
