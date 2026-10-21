use super::*;
use oauth_core::resource::AuraResource;
use user_service::use_cases::queries::authenticate_access_token::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenHandler, AuthenticateAccessTokenRequest,
    AuthenticateAccessTokenUseCase,
};

fn authorize_handler(ports: &FakePorts, policy: AuraResourcePolicy) -> impl AuthorizeUseCase {
    AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        policy,
    )
}

fn token_handler(
    ports: &FakePorts,
    policy: AuraResourcePolicy,
) -> impl TokenByAuthorizationCodeUseCase {
    TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        policy,
    )
}

fn token_request(
    ports: &FakePorts,
    secret: RawOAuthClientSecret,
    resource: Option<&str>,
) -> TokenByAuthorizationCodeRequest {
    let code = lock(&ports.0).code.clone().unwrap();
    TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: code.redirect_uri().clone(),
        resource: resource.map(str::to_owned),
        client_id: code.client_id(),
        client_secret: secret,
        code_verifier: OAuthCodeVerifier::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    }
}

async fn authorize(
    ports: &FakePorts,
    policy: AuraResourcePolicy,
    resource: Option<&str>,
) -> (RawOAuthClientSecret, UserId) {
    let secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&secret);
    lock(&ports.0).client = Some(client.clone());
    let user_id = UserId::new();
    let mut request = authorize_request(
        client.client_id(),
        HashSet::from([Scope::ProductListingsWrite]),
    );
    request.resource = resource.map(str::to_owned);
    authorize_handler(ports, policy)
        .execute(&context(Principal::User(user_id)), request)
        .await
        .unwrap();
    (secret, user_id)
}

#[tokio::test]
async fn should_bind_s256_grants_to_the_exact_identifier_without_changing_token_authority_or_expiry()
 {
    for policy in [
        AuraResourcePolicy::production(),
        AuraResourcePolicy::stage(),
    ] {
        for resource in policy.allowed_resources() {
            let ports = FakePorts::default();
            let (secret, user_id) = authorize(&ports, policy, Some(resource)).await;
            let code = lock(&ports.0).code.clone().unwrap();
            assert_eq!(resource, code.resource().unwrap().as_str());
            assert_eq!(user_id, code.user_id());
            assert!(code.expires() <= OffsetDateTime::now_utc() + time::Duration::minutes(10));
            let request = token_request(&ports, secret, Some(resource));
            let result = token_handler(&ports, policy)
                .execute(request.clone())
                .await
                .unwrap();
            assert_eq!(OAuthTokenType::Bearer, result.token_type);
            assert_eq!(None, result.expires);
            assert_eq!(HashSet::from([Scope::ProductListingsWrite]), result.scopes);
            let issued = lock(&ports.0).issued.clone().unwrap();
            assert_eq!(user_id, issued.user_id());
            assert_eq!(
                &AccessTokenOrigin::OAuth {
                    client_id: code.client_id()
                },
                issued.origin()
            );
            assert_eq!(issued.hashed_token(), &result.access_token.into());
            assert!(matches!(
                token_handler(&ports, policy).execute(request).await,
                Err(OAuthServiceError::AuthorizationCodeNotFound)
            ));
        }
    }
}

#[tokio::test]
async fn should_record_the_shared_aura_default_when_legacy_clients_omit_resource() {
    for policy in [
        AuraResourcePolicy::production(),
        AuraResourcePolicy::stage(),
    ] {
        let ports = FakePorts::default();
        let (secret, _) = authorize(&ports, policy, None).await;
        assert_eq!(
            policy.issuer(),
            lock(&ports.0)
                .code
                .as_ref()
                .unwrap()
                .resource()
                .unwrap()
                .as_str()
        );
        let result = token_handler(&ports, policy)
            .execute(token_request(&ports, secret, None))
            .await
            .unwrap();
        assert_eq!(None, result.expires);
        assert_eq!(HashSet::from([Scope::ProductListingsWrite]), result.scopes);
    }
}

#[tokio::test]
async fn should_inherit_the_mcp_binding_when_token_resource_is_omitted() {
    let policy = AuraResourcePolicy::production();
    let ports = FakePorts::default();
    let (secret, _) = authorize(&ports, policy, Some(policy.mcp_resource())).await;
    token_handler(&ports, policy)
        .execute(token_request(&ports, secret, None))
        .await
        .unwrap();
    assert!(lock(&ports.0).issued.is_some());
}

#[tokio::test]
async fn should_redeem_outstanding_codes_without_a_resource_using_the_shared_default() {
    let ports = FakePorts::default();
    let secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&secret);
    lock(&ports.0).code = Some(authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    ));
    lock(&ports.0).client = Some(client);
    token_handler(&ports, AuraResourcePolicy::stage())
        .execute(token_request(&ports, secret, None))
        .await
        .unwrap();
    assert!(lock(&ports.0).issued.is_some());
}

#[tokio::test]
async fn should_reject_foreign_and_other_environment_resources_before_authorization_or_exchange() {
    for resource in [
        "",
        "https://foreign.example",
        "https://api.stage.aura-historia.com",
        "https://mcp.stage.aura-historia.com/v1",
        "https://mcp.aura-historia.com/v1#fragment",
        "https://mcp.aura-historia.com/v1/tools",
    ] {
        let policy = AuraResourcePolicy::production();
        let ports = FakePorts::default();
        let mut request = authorize_request(OAuthClientId::new(), HashSet::new());
        request.resource = Some(resource.to_owned());
        assert!(
            matches!(
                authorize_handler(&ports, policy)
                    .execute(&first_party_context(), request)
                    .await,
                Err(OAuthServiceError::InvalidResource)
            ),
            "{resource}"
        );
        assert_eq!(0, lock(&ports.0).transaction_begins);
        let (secret, _) = authorize(&ports, policy, Some(policy.mcp_resource())).await;
        assert!(
            matches!(
                token_handler(&ports, policy)
                    .execute(token_request(&ports, secret, Some(resource)))
                    .await,
                Err(OAuthServiceError::InvalidResource)
            ),
            "{resource}"
        );
        let state = lock(&ports.0);
        assert_eq!(1, state.transaction_begins);
        assert!(state.code.is_some());
        assert!(state.issued.is_none());
    }
}

#[tokio::test]
async fn should_consume_the_code_and_reject_substituting_another_valid_shared_resource_alias() {
    let policy = AuraResourcePolicy::production();
    for (granted, requested) in [
        (Some(policy.mcp_resource()), policy.issuer()),
        (Some(policy.issuer()), policy.mcp_resource()),
        (None, policy.mcp_resource()),
    ] {
        let ports = FakePorts::default();
        let (secret, _) = authorize(&ports, policy, granted).await;
        assert!(matches!(
            token_handler(&ports, policy)
                .execute(token_request(&ports, secret, Some(requested)))
                .await,
            Err(OAuthServiceError::AuthorizationCodeResourceMismatch)
        ));
        let state = lock(&ports.0);
        assert!(state.code.is_none());
        assert!(state.issued.is_none());
        assert!(state.exchange.is_none());
        assert_eq!(2, state.transaction_commits);
    }
}

#[tokio::test]
async fn should_fail_closed_when_a_persisted_code_belongs_to_another_environment() {
    let ports = FakePorts::default();
    let policy = AuraResourcePolicy::stage();
    let (secret, _) = authorize(&ports, policy, Some(policy.mcp_resource())).await;
    assert_eq!(
        Some(AuraResource::StageMcp),
        lock(&ports.0).code.as_ref().unwrap().resource()
    );
    let result = token_handler(&ports, AuraResourcePolicy::production())
        .execute(token_request(&ports, secret, None))
        .await;
    assert!(matches!(
        result,
        Err(OAuthServiceError::InvalidPersistedState { .. })
    ));
    assert!(lock(&ports.0).issued.is_none());
    assert!(lock(&ports.0).code.is_some());
}

#[tokio::test]
async fn should_authenticate_mcp_tokens_with_shared_authentication_and_apply_database_revocation() {
    let ports = FakePorts::default();
    let policy = AuraResourcePolicy::production();
    let (secret, user_id) = authorize(&ports, policy, Some(policy.mcp_resource())).await;
    let token = token_handler(&ports, policy)
        .execute(token_request(
            &ports,
            secret.clone(),
            Some(policy.mcp_resource()),
        ))
        .await
        .unwrap();
    let request = AuthenticateAccessTokenRequest {
        hashed_token: token.access_token.clone().into(),
    };
    let authenticator = AuthenticateAccessTokenHandler::new(ports.clone());
    let authenticated = authenticator
        .execute(&context(Principal::Anonymous), request.clone())
        .await
        .unwrap();
    assert_eq!(user_id, authenticated.user_id);
    assert_eq!(
        HashSet::from([Scope::ProductListingsWrite]),
        authenticated.scopes
    );
    assert!(!authenticated.scopes.contains(&Scope::UsersWrite));
    let client_id = lock(&ports.0).client.as_ref().unwrap().client_id();
    RevokeTokenHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::System),
            RevokeTokenRequest {
                client_id,
                client_secret: secret,
                token: token.access_token,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        authenticator
            .execute(&context(Principal::Anonymous), request)
            .await,
        Err(AuthenticateAccessTokenError::NotFound)
    ));
}
