use crate::error::OAuthServiceError;
use crate::ports::{ThirdPartyExchangeCodeRepository, ThirdPartyExchangeCodeRepositoryFactory};
use crate::use_cases::token_by_authorization_code::{OAuthTokenType, TokenResponse};
use application::transaction::{Transaction, UnitOfWork};
use oauth_core::third_party_exchange_code::ThirdPartyExchangeCode;

#[async_trait::async_trait]
pub trait TokenByThirdPartyCodeUseCase: Send + Sync {
    async fn execute(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

pub struct TokenByThirdPartyCodeHandler<U, R> {
    unit_of_work: U,
    repository: R,
}
impl<U, R> TokenByThirdPartyCodeHandler<U, R> {
    pub fn new(unit_of_work: U, repository: R) -> Self {
        Self {
            unit_of_work,
            repository,
        }
    }
}
#[async_trait::async_trait]
impl<U, R> TokenByThirdPartyCodeUseCase for TokenByThirdPartyCodeHandler<U, R>
where
    U: UnitOfWork,
    R: ThirdPartyExchangeCodeRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let mut tx = self.unit_of_work.begin().await?;
        let grant = self
            .repository
            .in_transaction(&mut tx)
            .consume_by_code(code)
            .await?
            .ok_or(OAuthServiceError::ThirdPartyExchangeCodeNotFound)?;
        if grant.is_expired_at(time::OffsetDateTime::now_utc()) {
            tx.commit().await?;
            return Err(OAuthServiceError::ThirdPartyExchangeCodeExpired);
        }
        tx.commit().await?;
        Ok(TokenResponse {
            access_token: grant.access_token().clone(),
            token_type: OAuthTokenType::Bearer,
            expires: grant.access_token_expires(),
            scopes: grant.scopes().clone(),
            third_party_exchange_code: None,
        })
    }
}
