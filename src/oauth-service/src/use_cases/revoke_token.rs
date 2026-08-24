use crate::error::OAuthServiceError;
use crate::ports::OAuthClientRepositoryFactory;
use crate::use_cases::support::authenticate_client;
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use user_core::access_token::{HashedRawAccessToken, RawAccessToken, RawOAuthClientSecret};
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory};

#[derive(Debug, Clone, PartialEq)]
pub struct RevokeTokenRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
}

#[async_trait::async_trait]
pub trait RevokeTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError>;
}

pub struct RevokeTokenHandler<U, C, R> {
    unit_of_work: U,
    clients: C,
    access_tokens: R,
}
impl<U, C, R> RevokeTokenHandler<U, C, R> {
    pub fn new(unit_of_work: U, clients: C, access_tokens: R) -> Self {
        Self {
            unit_of_work,
            clients,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<U, C, R> RevokeTokenUseCase for RevokeTokenHandler<U, C, R>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        _context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError> {
        let mut tx = self.unit_of_work.begin().await?;
        authenticate_client(
            &mut self.clients.in_transaction(&mut tx),
            &request.client_id,
            &request.client_secret,
        )
        .await?;
        let hashed_token = HashedRawAccessToken::from(request.token);
        let token = self
            .access_tokens
            .in_transaction(&mut tx)
            .find_by_hashed_token(&hashed_token)
            .await?;
        if let Some(token) = token {
            self.access_tokens
                .in_transaction(&mut tx)
                .delete_by_id(token.value.user_id(), token.value.id())
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
