use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, OAuthClientRepository,
    OAuthClientRepositoryFactory,
};
use crate::use_cases::support::{AUTHORIZATION_CODE_TTL, append_query_params};
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::string_newtype;
use oauth_core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    RehydratedAuthorizationCodeState,
};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use user_core::access_token::Scope;
use user_core::user_id::UserId;

string_newtype!(OAuthState);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthResponseType {
    Code,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeRequest {
    pub response_type: OAuthResponseType,
    pub client_id: OAuthClientId,
    pub redirect_uri: url::Url,
    pub scope: HashSet<Scope>,
    pub state: Option<OAuthState>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeResponse {
    pub redirect_to: String,
}

#[async_trait::async_trait]
pub trait AuthorizeUseCase: Send + Sync {
    async fn execute(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError>;
}

pub struct AuthorizeHandler<U, C, A> {
    unit_of_work: U,
    clients: C,
    codes: A,
}
impl<U, C, A> AuthorizeHandler<U, C, A> {
    pub fn new(unit_of_work: U, clients: C, codes: A) -> Self {
        Self {
            unit_of_work,
            clients,
            codes,
        }
    }
}
#[async_trait::async_trait]
impl<U, C, A> AuthorizeUseCase for AuthorizeHandler<U, C, A>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError> {
        let mut tx = self.unit_of_work.begin().await?;
        let client = self
            .clients
            .in_transaction(&mut tx)
            .find_by_id(request.client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)?
            .value;
        if !client.redirect_uris().contains(&request.redirect_uri) {
            return Err(OAuthServiceError::InvalidRedirectUri);
        }
        if !request.scope.is_subset(client.scopes()) {
            return Err(OAuthServiceError::InvalidScope);
        }
        let now = OffsetDateTime::now_utc();
        let code = AuthorizationCode::create(RehydratedAuthorizationCodeState {
            code: OAuthAuthorizationCode::new(),
            client_id: request.client_id,
            user_id: *user_id,
            redirect_uri: request.redirect_uri.clone(),
            scopes: request.scope,
            code_challenge: request.code_challenge,
            code_challenge_method: request.code_challenge_method,
            expires: now + AUTHORIZATION_CODE_TTL,
        });
        self.codes
            .in_transaction(&mut tx)
            .insert(code.clone())
            .await?;
        let mut params = HashMap::from([("code", code.code().to_string())]);
        if let Some(state) = request.state {
            params.insert("state", state.as_ref().to_owned());
        }
        let response = AuthorizeResponse {
            redirect_to: append_query_params(&request.redirect_uri, params),
        };
        tx.commit().await?;
        Ok(response)
    }
}
