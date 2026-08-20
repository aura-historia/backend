use application::operation_context::{
    CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    OAuthCodeVerifier,
};
use oauth_core::client::{OAuthClient, OAuthClientName};
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use oauth_service::error::OAuthServiceError;
use oauth_service::ports::*;
use oauth_service::use_cases::{
    AuthorizeHandler, AuthorizeRequest, AuthorizeUseCase, CreateOAuthClientCommand,
    CreateOAuthClientHandler, CreateOAuthClientUseCase, DeleteOAuthClientHandler,
    DeleteOAuthClientUseCase, GetOAuthClientHandler, GetOAuthClientUseCase, IntrospectTokenHandler,
    IntrospectTokenRequest, IntrospectTokenUseCase, ListOAuthClientsHandler,
    ListOAuthClientsUseCase, OAuthGrantType, OAuthResponseType, OAuthState, OAuthTokenType,
    RevokeTokenHandler, RevokeTokenRequest, RevokeTokenUseCase, TokenByAuthorizationCodeHandler,
    TokenByAuthorizationCodeRequest, TokenByAuthorizationCodeUseCase, TokenByThirdPartyCodeHandler,
    TokenByThirdPartyCodeUseCase, UpdateOAuthClientCommand, UpdateOAuthClientHandler,
    UpdateOAuthClientUseCase,
};
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use time::OffsetDateTime;
use user_core::access_token::{
    AccessTokenOrigin, HashedRawOAuthClientSecret, RawAccessToken, RawOAuthClientSecret, Scope,
};
use user_core::user_id::UserId;

#[derive(Default)]
struct State {
    client: Option<OAuthClient>,
    raw_secret: Option<RawOAuthClientSecret>,
    code: Option<AuthorizationCode>,
    exchange: Option<ThirdPartyExchangeCodeGrant>,
    issued: Option<IssuedAccessToken>,
    deleted_raw: usize,
}

#[derive(Clone, Default)]
struct FakePorts(Arc<Mutex<State>>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn ctx() -> OperationContext {
    OperationContext {
        principal: Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
        },
        request_id: RequestId::new("req"),
        correlation_id: CorrelationId::new("corr"),
    }
}

fn url(value: &str) -> url::Url {
    url::Url::parse(value).unwrap_or_else(|_| unreachable!())
}

fn client_with_secret(raw: &RawOAuthClientSecret) -> OAuthClient {
    let now =
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    OAuthClient {
        client_id: OAuthClientId::new(),
        hashed_client_secret: HashedRawOAuthClientSecret::from(raw.clone()),
        name: OAuthClientName::from("Test Client"),
        redirect_uris: HashSet::from([url("https://client.example/callback")]),
        tos_uri: url("https://client.example/tos"),
        policy_uri: url("https://client.example/policy"),
        client_uri: url("https://client.example"),
        logo_uri: url("https://client.example/logo.png"),
        scopes: HashSet::from([Scope::ProductsWrite]),
        created: now,
        updated: now,
    }
}

#[async_trait::async_trait]
impl OAuthClientReader for FakePorts {
    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError> {
        Ok(lock(&self.0).client.clone().into_iter().collect())
    }
}

#[async_trait::async_trait]
impl OAuthClientRepository for FakePorts {
    async fn find_by_client_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError> {
        Ok(lock(&self.0)
            .client
            .clone()
            .filter(|client| client.client_id == *client_id))
    }

    async fn insert(
        &self,
        client: OAuthClient,
        raw_secret: RawOAuthClientSecret,
    ) -> Result<(), OAuthClientRepositoryError> {
        let mut s = lock(&self.0);
        s.client = Some(client);
        s.raw_secret = Some(raw_secret);
        Ok(())
    }
    async fn update(
        &self,
        client_id: &OAuthClientId,
        patch: OAuthClientPatch,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError> {
        let mut s = lock(&self.0);
        let Some(client) = &mut s.client else {
            return Ok(None);
        };
        if client.client_id != *client_id {
            return Ok(None);
        } else if let Some(name) = patch.name {
            client.name = name;
        }
        client.updated = patch.updated;
        Ok(Some(client.clone()))
    }
    async fn delete(&self, client_id: &OAuthClientId) -> Result<(), OAuthClientRepositoryError> {
        let mut s = lock(&self.0);
        if s.client.as_ref().is_some_and(|c| c.client_id == *client_id) {
            s.client = None;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl AuthorizationCodeRepository for FakePorts {
    async fn insert(&self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        lock(&self.0).code = Some(code);
        Ok(())
    }
    async fn find_by_code(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError> {
        Ok(lock(&self.0).code.clone().filter(|c| c.code == *code))
    }
    async fn delete(&self, code: &OAuthAuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        let mut s = lock(&self.0);
        if s.code.as_ref().is_some_and(|c| c.code == *code) {
            s.code = None;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ThirdPartyExchangeCodeRepository for FakePorts {
    async fn insert(
        &self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError> {
        lock(&self.0).exchange = Some(grant);
        Ok(())
    }
    async fn find_by_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError> {
        Ok(lock(&self.0).exchange.clone().filter(|g| g.code == *code))
    }
    async fn delete(&self, code: &ThirdPartyExchangeCode) -> Result<(), OAuthCodeRepositoryError> {
        let mut s = lock(&self.0);
        if s.exchange.as_ref().is_some_and(|g| g.code == *code) {
            s.exchange = None;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl OAuthAccessTokenGateway for FakePorts {
    async fn issue(
        &self,
        token: NewOAuthAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError> {
        let issued = IssuedAccessToken {
            raw: RawAccessToken::new(),
            expires: None,
            scopes: token.scopes,
            user_id: token.user_id,
            origin: AccessTokenOrigin::OAuth {
                client_id: token.client_id,
            },
            issued_at: Some(OffsetDateTime::UNIX_EPOCH),
        };
        lock(&self.0).issued = Some(issued.clone());
        Ok(issued)
    }
    async fn delete_raw(&self, raw: &RawAccessToken) -> Result<(), OAuthAccessTokenGatewayError> {
        let mut s = lock(&self.0);
        if s.issued.as_ref().is_some_and(|t| &t.raw == raw) {
            s.deleted_raw += 1;
            Ok(())
        } else {
            Err(OAuthAccessTokenGatewayError::NotFound)
        }
    }
    async fn find_raw(
        &self,
        raw: &RawAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError> {
        lock(&self.0)
            .issued
            .clone()
            .filter(|t| &t.raw == raw)
            .ok_or(OAuthAccessTokenGatewayError::NotFound)
    }
}

#[tokio::test]
async fn should_cover_client_crud_use_cases() {
    let ports = FakePorts::default();
    let create = CreateOAuthClientHandler::new(ports.clone());
    let result = create
        .execute(
            &ctx(),
            CreateOAuthClientCommand {
                name: OAuthClientName::from("A"),
                redirect_uris: HashSet::from([url("https://client.example/callback")]),
                tos_uri: url("https://client.example/tos"),
                policy_uri: url("https://client.example/policy"),
                client_uri: url("https://client.example"),
                logo_uri: url("https://client.example/logo.png"),
                scopes: HashSet::from([Scope::ProductsWrite]),
            },
        )
        .await
        .unwrap();
    assert!(
        result
            .raw_client_secret
            .check(&result.client.hashed_client_secret)
    );
    assert_eq!(
        1,
        ListOAuthClientsHandler::new(ports.clone())
            .execute()
            .await
            .unwrap()
            .len()
    );
    assert_eq!(
        result.client.client_id,
        GetOAuthClientHandler::new(ports.clone())
            .execute(&result.client.client_id)
            .await
            .unwrap()
            .client_id
    );
    let updated = UpdateOAuthClientHandler::new(ports.clone())
        .execute(
            &ctx(),
            &result.client.client_id,
            UpdateOAuthClientCommand {
                name: Some(OAuthClientName::from("B")),
                redirect_uris: None,
                tos_uri: None,
                policy_uri: None,
                client_uri: None,
                logo_uri: None,
                scopes: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(OAuthClientName::from("B"), updated.name);
    DeleteOAuthClientHandler::new(ports.clone())
        .execute(&ctx(), &result.client.client_id)
        .await
        .unwrap();
    assert!(
        ListOAuthClientsHandler::new(ports.clone())
            .execute()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn should_reject_invalid_client_metadata() {
    let err = CreateOAuthClientHandler::new(FakePorts::default())
        .execute(
            &ctx(),
            CreateOAuthClientCommand {
                name: OAuthClientName::from("A"),
                redirect_uris: HashSet::new(),
                tos_uri: url("https://client.example/tos"),
                policy_uri: url("https://client.example/policy"),
                client_uri: url("https://client.example"),
                logo_uri: url("https://client.example/logo.png"),
                scopes: HashSet::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, OAuthServiceError::InvalidClientMetadata(_)));
}

#[tokio::test]
async fn should_authorize_and_exchange_tokens() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());
    let authorize = AuthorizeHandler::new(ports.clone(), ports.clone());
    let auth = authorize
        .execute(
            &UserId::new(),
            AuthorizeRequest {
                response_type: OAuthResponseType::Code,
                client_id: client.client_id,
                redirect_uri: url("https://client.example/callback"),
                scope: HashSet::from([Scope::ProductsWrite]),
                state: Some(OAuthState::from("state-1")),
                code_challenge: OAuthCodeChallenge::from(
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                ),
                code_challenge_method: CodeChallengeMethod::S256,
            },
        )
        .await
        .unwrap();
    assert!(auth.redirect_to.contains("state=state-1"));
    let code = lock(&ports.0).code.clone().unwrap().code;
    let token = TokenByAuthorizationCodeHandler::new(
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code,
        redirect_uri: url("https://client.example/callback"),
        client_id: client.client_id,
        client_secret: raw_secret,
        code_verifier: OAuthCodeVerifier::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    })
    .await
    .unwrap();
    assert!(matches!(token.token_type, OAuthTokenType::Bearer));
    assert!(token.third_party_exchange_code.is_some());
    assert!(lock(&ports.0).code.is_none());
}

#[tokio::test]
async fn should_reject_authorize_with_invalid_scope() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());
    let err = AuthorizeHandler::new(ports.clone(), ports.clone())
        .execute(
            &UserId::new(),
            AuthorizeRequest {
                response_type: OAuthResponseType::Code,
                client_id: client.client_id,
                redirect_uri: url("https://client.example/callback"),
                scope: HashSet::from([Scope::ShopsRead]),
                state: None,
                code_challenge: OAuthCodeChallenge::from("challenge"),
                code_challenge_method: CodeChallengeMethod::S256,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, OAuthServiceError::InvalidScope));
}

#[tokio::test]
async fn should_exchange_third_party_code_once() {
    let ports = FakePorts::default();
    let grant = ThirdPartyExchangeCodeGrant {
        code: ThirdPartyExchangeCode::new(),
        access_token: RawAccessToken::new(),
        access_token_expires: None,
        scopes: HashSet::from([Scope::ProductsWrite]),
        expires: OffsetDateTime::now_utc() + time::Duration::minutes(1),
        created: OffsetDateTime::now_utc(),
    };
    lock(&ports.0).exchange = Some(grant.clone());
    let response = TokenByThirdPartyCodeHandler::new(ports.clone())
        .execute(&grant.code)
        .await
        .unwrap();
    assert_eq!(grant.access_token, response.access_token);
    assert!(lock(&ports.0).exchange.is_none());
}

#[tokio::test]
async fn should_revoke_and_introspect_tokens() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let issued = IssuedAccessToken {
        raw: RawAccessToken::new(),
        expires: None,
        scopes: HashSet::from([Scope::ProductsWrite]),
        user_id: UserId::new(),
        origin: AccessTokenOrigin::OAuth {
            client_id: client.client_id,
        },
        issued_at: Some(OffsetDateTime::UNIX_EPOCH),
    };
    {
        let mut s = lock(&ports.0);
        s.client = Some(client.clone());
        s.issued = Some(issued.clone());
    }
    let active = IntrospectTokenHandler::new(ports.clone(), ports.clone())
        .execute(IntrospectTokenRequest {
            token: issued.raw.clone(),
            client_id: client.client_id,
            client_secret: raw_secret.clone(),
        })
        .await
        .unwrap();
    assert!(active.active);
    RevokeTokenHandler::new(ports.clone(), ports.clone())
        .execute(
            &ctx(),
            RevokeTokenRequest {
                token: issued.raw.clone(),
                client_id: client.client_id,
                client_secret: raw_secret.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(1, lock(&ports.0).deleted_raw);
    let inactive = IntrospectTokenHandler::new(ports.clone(), ports.clone())
        .execute(IntrospectTokenRequest {
            token: RawAccessToken::new(),
            client_id: client.client_id,
            client_secret: raw_secret,
        })
        .await
        .unwrap();
    assert!(!inactive.active);
}
