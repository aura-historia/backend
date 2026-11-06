//! Connections, rotating refresh tokens, revocation and disconnection of metadata document
//! clients.
use super::*;
use oauth_core::metadata_document_client::DeniedMetadataDocumentHosts;
use oauth_core::refresh_token::{
    IssuedRefreshToken, RawRefreshToken, RehydratedRefreshTokenFamilyState,
};
use oauth_core::resource::AuraResource;
use oauth_service::use_cases::{
    DisconnectOAuthClientCommand, DisconnectOAuthClientHandler, DisconnectOAuthClientUseCase,
    GetMetadataDocumentConsentClientHandler, GetMetadataDocumentConsentClientRequest,
    GetMetadataDocumentConsentClientUseCase, TokenByRefreshTokenHandler,
    TokenByRefreshTokenRequest, TokenByRefreshTokenUseCase, TokenResponse,
};
use time::Duration;

const CLIENT_ID: &str = "https://agent.example/oauth/client.json";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn client_id() -> MetadataDocumentClientId {
    MetadataDocumentClientId::try_from(CLIENT_ID).unwrap()
}

fn enabled() -> MetadataDocumentClientPolicy {
    MetadataDocumentClientPolicy::Enabled {
        denied_hosts: DeniedMetadataDocumentHosts::default(),
    }
}

/// Serves the agent's document; `grant_types` decides whether it asks for refresh tokens.
#[derive(Clone)]
struct AgentDocument {
    grant_types: Vec<&'static str>,
}

#[async_trait::async_trait]
impl ClientMetadataDocumentSource for AgentDocument {
    async fn load(
        &self,
        _client_id: &MetadataDocumentClientId,
    ) -> Result<ClientMetadataDocumentClaims, ClientMetadataDocumentLoadError> {
        Ok(ClientMetadataDocumentClaims {
            client_id: Some(CLIENT_ID.to_owned()),
            client_name: Some("Example Agent".to_owned()),
            redirect_uris: Some(vec!["http://127.0.0.1/callback".to_owned()]),
            token_endpoint_auth_method: Some("none".to_owned()),
            grant_types: Some(self.grant_types.iter().map(|&value| value.into()).collect()),
            response_types: None,
            client_uri: None,
            logo_uri: None,
            declares_client_secret: false,
        })
    }
}

fn refreshing_document() -> AgentDocument {
    AgentDocument {
        grant_types: vec!["authorization_code", "refresh_token"],
    }
}

/// Approves the agent for `user` and exchanges the code.
async fn sign_in(
    ports: &FakePorts,
    document: AgentDocument,
    user: UserId,
    scopes: &[Scope],
    offline_access: bool,
) -> TokenResponse {
    AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        MetadataDocumentClients::new(enabled(), document),
        AuraResourcePolicy::production(),
    )
    .execute(
        &context(Principal::User(user)),
        AuthorizeRequest {
            response_type: OAuthResponseType::Code,
            client_id: client_id().into(),
            redirect_uri: url("http://127.0.0.1:49152/callback"),
            resource: Some("https://mcp.aura-historia.com/v1".to_owned()),
            scope: scopes.iter().copied().collect(),
            offline_access,
            state: None,
            code_challenge: OAuthCodeChallenge::from(CHALLENGE),
            code_challenge_method: CodeChallengeMethod::S256,
        },
    )
    .await
    .unwrap();
    let code = lock(&ports.0).code.clone().unwrap();
    exchange(ports, &code).await.unwrap()
}

async fn exchange(
    ports: &FakePorts,
    code: &AuthorizationCode,
) -> Result<TokenResponse, OAuthServiceError> {
    TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        token_exchange_stores(ports),
        credentials(enabled()),
        AuraResourcePolicy::production(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: code.redirect_uri().clone(),
        resource: None,
        client: OAuthTokenClientAuthentication::None {
            client_id: client_id(),
        },
        code_verifier: OAuthCodeVerifier::from(VERIFIER),
    })
    .await
}

async fn refresh_with(
    ports: &FakePorts,
    policy: MetadataDocumentClientPolicy,
    request: TokenByRefreshTokenRequest,
) -> Result<TokenResponse, OAuthServiceError> {
    TokenByRefreshTokenHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        credentials(policy),
        AuraResourcePolicy::production(),
    )
    .execute(request)
    .await
}

fn refresh_request(token: &RawRefreshToken) -> TokenByRefreshTokenRequest {
    TokenByRefreshTokenRequest {
        refresh_token: token.clone(),
        client_id: client_id(),
        scope: None,
        resource: None,
    }
}

async fn refresh(
    ports: &FakePorts,
    token: &RawRefreshToken,
) -> Result<TokenResponse, OAuthServiceError> {
    refresh_with(ports, enabled(), refresh_request(token)).await
}

fn connection(ports: &FakePorts) -> OAuthConnection {
    lock(&ports.0)
        .connections
        .first()
        .cloned()
        .expect("a connection")
}

/// Moves when every refresh token of the family was replaced into the past.
fn age_replacements(ports: &FakePorts, by: Duration) {
    for (_, _, replaced_at) in &mut lock(&ports.0).refresh_tokens {
        *replaced_at = replaced_at.map(|at| at - by);
    }
}

/// Rewrites the only family's lifetime limits, keeping its tokens.
fn expire_family(ports: &FakePorts, idle: OffsetDateTime, absolute: OffsetDateTime) {
    let mut state = lock(&ports.0);
    let family = state.families.pop().expect("a family");
    state.families.push(RefreshTokenFamily::rehydrate(
        RehydratedRefreshTokenFamilyState {
            id: family.id(),
            connection_id: family.connection_id(),
            scopes: family.scopes().clone(),
            resource: family.resource(),
            created: family.created(),
            absolute_expires: absolute,
            current: IssuedRefreshToken {
                expires: idle.min(absolute),
                ..family.current().clone()
            },
        },
    ));
}

#[tokio::test]
async fn should_start_a_connection_and_refresh_token_family_when_the_document_asks_for_them() {
    let ports = FakePorts::default();
    let user = UserId::new();

    let token = sign_in(
        &ports,
        refreshing_document(),
        user,
        &[Scope::UsersRead],
        false,
    )
    .await;

    let refresh_token = token
        .refresh_token
        .expect("the document lists refresh_token");
    let state = lock(&ports.0).clone();
    let connection = state.connections.first().unwrap();
    assert_eq!(user, connection.user_id());
    assert_eq!("Example Agent", connection.client_name().as_ref());
    assert_eq!(&HashSet::from([Scope::UsersRead]), connection.scopes());
    let family = state.families.first().unwrap();
    assert_eq!(connection.id(), family.connection_id());
    assert_eq!(AuraResource::ProductionMcp, family.resource());
    assert!(refresh_token.check(&family.current().hashed_token));
    assert!(matches!(
        state.issued.unwrap().origin(),
        AccessTokenOrigin::OAuthMetadataDocument { connection_id, .. } if *connection_id == connection.id()
    ));
}

#[tokio::test]
async fn should_issue_refresh_tokens_for_offline_access_and_never_unasked() {
    let without_grant = AgentDocument {
        grant_types: vec!["authorization_code"],
    };
    let ports = FakePorts::default();
    let unasked = sign_in(
        &ports,
        without_grant.clone(),
        UserId::new(),
        &[Scope::UsersRead],
        false,
    )
    .await;
    assert_eq!(None, unasked.refresh_token);
    assert!(lock(&ports.0).families.is_empty());

    let ports = FakePorts::default();
    let offline = sign_in(
        &ports,
        without_grant,
        UserId::new(),
        &[Scope::UsersRead],
        true,
    )
    .await;
    assert!(offline.refresh_token.is_some());
}

#[tokio::test]
async fn should_refuse_offline_access_from_registered_clients() {
    let ports = FakePorts::default();
    let client = client_with_secret(&RawOAuthClientSecret::new());
    lock(&ports.0).client = Some(client.clone());

    let error = AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        no_metadata_document_clients(),
        AuraResourcePolicy::production(),
    )
    .execute(
        &first_party_context(),
        AuthorizeRequest {
            offline_access: true,
            ..authorize_request(
                client.client_id(),
                HashSet::from([Scope::ProductListingsWrite]),
            )
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(error, OAuthServiceError::InvalidScope));
    assert!(lock(&ports.0).code.is_none());
}

#[tokio::test]
async fn should_add_step_up_scopes_to_the_connection_and_show_them_on_consent() {
    let ports = FakePorts::default();
    let user = UserId::new();
    sign_in(
        &ports,
        refreshing_document(),
        user,
        &[Scope::UsersRead],
        false,
    )
    .await;
    let first = connection(&ports);

    sign_in(
        &ports,
        refreshing_document(),
        user,
        &[Scope::UsersRead, Scope::WatchlistWrite],
        false,
    )
    .await;

    let state = lock(&ports.0).clone();
    assert_eq!(
        1,
        state.connections.len(),
        "one connection per user and client"
    );
    let connection = state.connections.first().unwrap();
    assert_eq!(first.id(), connection.id());
    assert_eq!(
        &HashSet::from([Scope::UsersRead, Scope::WatchlistWrite]),
        connection.scopes()
    );
    assert_eq!(
        2,
        state.families.len(),
        "each sign-in starts its own family"
    );
    let consent = GetMetadataDocumentConsentClientHandler::new(
        MetadataDocumentClients::new(enabled(), refreshing_document()),
        ports.clone(),
    )
    .execute(
        &context(Principal::User(user)),
        GetMetadataDocumentConsentClientRequest {
            client_id: client_id(),
            redirect_uri: url("http://127.0.0.1:1/callback"),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        HashSet::from([Scope::UsersRead, Scope::WatchlistWrite]),
        consent.granted_scopes
    );
}

#[tokio::test]
async fn should_rotate_the_refresh_token_on_every_use() {
    let ports = FakePorts::default();
    let signed_in = sign_in(
        &ports,
        refreshing_document(),
        UserId::new(),
        &[Scope::UsersRead, Scope::WatchlistRead],
        false,
    )
    .await;
    let first = signed_in.refresh_token.unwrap();
    let issued_before = connection(&ports).last_used();

    let refreshed = refresh(&ports, &first).await.unwrap();

    let second = refreshed.refresh_token.clone().expect("rotation");
    assert_ne!(String::from(first.clone()), String::from(second.clone()));
    assert_eq!(
        HashSet::from([Scope::UsersRead, Scope::WatchlistRead]),
        refreshed.scopes
    );
    assert!(
        refreshed
            .expires
            .is_some_and(|expires| expires <= OffsetDateTime::now_utc() + Duration::hours(1))
    );
    assert!(
        refreshed
            .access_token
            .check(lock(&ports.0).issued.as_ref().unwrap().hashed_token())
    );
    assert!(connection(&ports).last_used() >= issued_before);
    let replaced = lock(&ports.0)
        .refresh_tokens
        .iter()
        .filter(|(_, _, replaced_at)| replaced_at.is_some())
        .count();
    assert_eq!(1, replaced);
    assert!(
        refresh(&ports, &second).await.is_ok(),
        "the new token works"
    );
}

#[tokio::test]
async fn should_reject_the_loser_of_a_concurrent_refresh_without_revoking_the_family() {
    let ports = FakePorts::default();
    let first = sign_in(&ports, refreshing_document(), UserId::new(), &[], false)
        .await
        .refresh_token
        .unwrap();
    let winner = refresh(&ports, &first)
        .await
        .unwrap()
        .refresh_token
        .unwrap();

    let loser = refresh(&ports, &first).await.unwrap_err();

    assert!(matches!(loser, OAuthServiceError::RefreshTokenReplaced));
    assert_eq!(1, lock(&ports.0).families.len());
    assert!(lock(&ports.0).issued.is_some());
    assert!(refresh(&ports, &winner).await.is_ok());
}

#[tokio::test]
async fn should_revoke_the_family_and_the_connections_access_tokens_when_a_token_is_reused() {
    let ports = FakePorts::default();
    let user = UserId::new();
    let first = sign_in(&ports, refreshing_document(), user, &[], false)
        .await
        .refresh_token
        .unwrap();
    let current = refresh(&ports, &first)
        .await
        .unwrap()
        .refresh_token
        .unwrap();
    age_replacements(&ports, Duration::minutes(1));
    let commits = lock(&ports.0).transaction_commits;

    let reused = refresh(&ports, &first).await.unwrap_err();

    assert!(matches!(reused, OAuthServiceError::RefreshTokenReused));
    let state = lock(&ports.0).clone();
    assert_eq!(commits + 1, state.transaction_commits, "revocation commits");
    assert!(state.families.is_empty());
    assert!(state.refresh_tokens.is_empty());
    assert!(state.issued.is_none());
    let connection_id = state.connections.first().unwrap().id();
    assert_eq!(
        vec![(user, OAuthTokenHolder::Connection(connection_id))],
        state.revoked_holders
    );
    assert!(matches!(
        refresh(&ports, &current).await,
        Err(OAuthServiceError::RefreshTokenInvalid)
    ));
}

#[tokio::test]
async fn should_reject_a_family_past_its_idle_or_absolute_limit() {
    let past = OffsetDateTime::now_utc() - Duration::seconds(1);
    let future = OffsetDateTime::now_utc() + Duration::days(1);
    for (idle, absolute) in [(past, future), (future, past)] {
        let ports = FakePorts::default();
        let token = sign_in(&ports, refreshing_document(), UserId::new(), &[], false)
            .await
            .refresh_token
            .unwrap();
        expire_family(&ports, idle, absolute);
        let issued = lock(&ports.0).issued.clone();

        let error = refresh(&ports, &token).await.unwrap_err();

        assert!(matches!(error, OAuthServiceError::RefreshTokenExpired));
        assert_eq!(issued, lock(&ports.0).issued, "no new access token");
    }
}

#[tokio::test]
async fn should_narrow_but_never_widen_scopes_or_change_the_resource_on_refresh() {
    let ports = FakePorts::default();
    let mut token = sign_in(
        &ports,
        refreshing_document(),
        UserId::new(),
        &[Scope::UsersRead, Scope::WatchlistRead],
        false,
    )
    .await
    .refresh_token
    .unwrap();

    let narrowed = refresh_with(
        &ports,
        enabled(),
        TokenByRefreshTokenRequest {
            scope: Some(HashSet::from([Scope::UsersRead])),
            resource: Some("https://mcp.aura-historia.com/v1".to_owned()),
            ..refresh_request(&token)
        },
    )
    .await
    .unwrap();
    assert_eq!(HashSet::from([Scope::UsersRead]), narrowed.scopes);
    token = narrowed.refresh_token.unwrap();

    for (request, expected) in [
        (
            TokenByRefreshTokenRequest {
                scope: Some(HashSet::from([Scope::UsersWrite])),
                ..refresh_request(&token)
            },
            "scope",
        ),
        (
            TokenByRefreshTokenRequest {
                resource: Some("https://api.aura-historia.com".to_owned()),
                ..refresh_request(&token)
            },
            "resource",
        ),
        (
            TokenByRefreshTokenRequest {
                resource: Some("https://foreign.example".to_owned()),
                ..refresh_request(&token)
            },
            "target",
        ),
    ] {
        let error = refresh_with(&ports, enabled(), request).await.unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (OAuthServiceError::InvalidScope, "scope")
                    | (OAuthServiceError::RefreshTokenResourceMismatch, "resource")
                    | (OAuthServiceError::InvalidResource, "target")
            ),
            "{expected}: {error:?}"
        );
    }
    assert!(
        refresh(&ports, &token).await.is_ok(),
        "rejected requests leave the token current"
    );
}

#[tokio::test]
async fn should_treat_another_clients_or_an_unknown_token_as_invalid_without_side_effects() {
    let ports = FakePorts::default();
    let token = sign_in(&ports, refreshing_document(), UserId::new(), &[], false)
        .await
        .refresh_token
        .unwrap();
    let before = lock(&ports.0).clone();

    for request in [
        TokenByRefreshTokenRequest {
            client_id: MetadataDocumentClientId::try_from("https://other.example/client.json")
                .unwrap(),
            ..refresh_request(&token)
        },
        refresh_request(&RawRefreshToken::new()),
    ] {
        let error = refresh_with(&ports, enabled(), request).await.unwrap_err();
        assert!(matches!(error, OAuthServiceError::RefreshTokenInvalid));
    }
    let after = lock(&ports.0).clone();
    assert_eq!(before.refresh_tokens, after.refresh_tokens);
    assert_eq!(before.issued, after.issued);
    let denied = MetadataDocumentClientPolicy::Enabled {
        denied_hosts: DeniedMetadataDocumentHosts::parse(["agent.example"]).unwrap(),
    };
    for policy in [MetadataDocumentClientPolicy::Disabled, denied] {
        let error = refresh_with(&ports, policy, refresh_request(&token))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OAuthServiceError::InvalidMetadataDocumentClient { .. }
        ));
    }
}

fn revoke_handler(ports: &FakePorts) -> impl RevokeTokenUseCase {
    RevokeTokenHandler::new(
        FakeUnitOfWork(ports.clone()),
        revocation_stores(ports),
        credentials(enabled()),
    )
}

fn public_revocation(token: RevocableToken) -> RevokeTokenRequest {
    RevokeTokenRequest {
        token,
        client: OAuthTokenClientAuthentication::None {
            client_id: client_id(),
        },
    }
}

#[tokio::test]
async fn should_end_the_connection_when_a_client_revokes_its_refresh_token() {
    let ports = FakePorts::default();
    let token = sign_in(&ports, refreshing_document(), UserId::new(), &[], false)
        .await
        .refresh_token
        .unwrap();

    revoke_handler(&ports)
        .execute(
            &context(Principal::System),
            public_revocation(RevocableToken::RefreshToken(token.clone())),
        )
        .await
        .unwrap();

    let state = lock(&ports.0).clone();
    assert!(state.connections.is_empty());
    assert!(state.families.is_empty());
    assert!(state.issued.is_none());
    assert!(
        revoke_handler(&ports)
            .execute(
                &context(Principal::System),
                public_revocation(RevocableToken::RefreshToken(token)),
            )
            .await
            .is_ok(),
        "revoking again is a no-op"
    );
}

#[tokio::test]
async fn should_let_a_public_client_revoke_only_its_own_tokens() {
    let ports = FakePorts::default();
    let signed_in = sign_in(&ports, refreshing_document(), UserId::new(), &[], false).await;

    revoke_handler(&ports)
        .execute(
            &context(Principal::System),
            public_revocation(RevocableToken::AccessToken(signed_in.access_token)),
        )
        .await
        .unwrap();
    assert!(lock(&ports.0).issued.is_none());
    assert_eq!(1, lock(&ports.0).connections.len(), "the connection stays");

    let other = MetadataDocumentClientId::try_from("https://other.example/client.json").unwrap();
    let error = revoke_handler(&ports)
        .execute(
            &context(Principal::System),
            RevokeTokenRequest {
                token: RevocableToken::RefreshToken(signed_in.refresh_token.clone().unwrap()),
                client: OAuthTokenClientAuthentication::None { client_id: other },
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, OAuthServiceError::Forbidden));

    let client = client_with_secret(&RawOAuthClientSecret::new());
    let secret = RawOAuthClientSecret::new();
    let client = OAuthClient::create(RehydratedOAuthClientState {
        client_id: client.client_id(),
        hashed_client_secret: HashedRawOAuthClientSecret::from(secret.clone()),
        name: client.name().clone(),
        redirect_uris: client.redirect_uris().clone(),
        tos_uri: client.tos_uri().clone(),
        policy_uri: client.policy_uri().clone(),
        client_uri: client.client_uri().clone(),
        logo_uri: client.logo_uri().clone(),
        scopes: client.scopes().clone(),
    });
    lock(&ports.0).client = Some(client.clone());
    let error = revoke_handler(&ports)
        .execute(
            &context(Principal::System),
            RevokeTokenRequest {
                token: RevocableToken::RefreshToken(signed_in.refresh_token.unwrap()),
                client: OAuthTokenClientAuthentication::ClientSecretPost {
                    client_id: client.client_id(),
                    client_secret: secret,
                },
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, OAuthServiceError::Forbidden));
    assert_eq!(1, lock(&ports.0).families.len());
}

fn disconnect_handler(ports: &FakePorts) -> impl DisconnectOAuthClientUseCase {
    DisconnectOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
}

#[tokio::test]
async fn should_disconnect_a_metadata_document_client_with_all_its_credentials() {
    let ports = FakePorts::default();
    let user = UserId::new();
    let token = sign_in(&ports, refreshing_document(), user, &[], false)
        .await
        .refresh_token
        .unwrap();
    // An approval whose code the client has not exchanged yet.
    AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        MetadataDocumentClients::new(enabled(), refreshing_document()),
        AuraResourcePolicy::production(),
    )
    .execute(
        &context(Principal::User(user)),
        AuthorizeRequest {
            response_type: OAuthResponseType::Code,
            client_id: client_id().into(),
            redirect_uri: url("http://127.0.0.1:49152/callback"),
            resource: None,
            scope: HashSet::new(),
            offline_access: false,
            state: None,
            code_challenge: OAuthCodeChallenge::from(CHALLENGE),
            code_challenge_method: CodeChallengeMethod::S256,
        },
    )
    .await
    .unwrap();
    let pending = lock(&ports.0).code.clone().unwrap();

    disconnect_handler(&ports)
        .execute(
            &context(Principal::User(user)),
            DisconnectOAuthClientCommand {
                user_id: user,
                client_id: client_id().into(),
            },
        )
        .await
        .unwrap();

    let state = lock(&ports.0).clone();
    assert!(state.connections.is_empty());
    assert!(state.families.is_empty());
    assert!(state.issued.is_none());
    assert!(state.code.is_none(), "the pending approval is withdrawn");
    assert!(matches!(
        refresh(&ports, &token).await,
        Err(OAuthServiceError::RefreshTokenInvalid)
    ));
    assert!(matches!(
        exchange(&ports, &pending).await,
        Err(OAuthServiceError::AuthorizationCodeNotFound)
    ));
    assert!(
        disconnect_handler(&ports)
            .execute(
                &context(Principal::User(user)),
                DisconnectOAuthClientCommand {
                    user_id: user,
                    client_id: client_id().into(),
                },
            )
            .await
            .is_ok(),
        "disconnecting again is a no-op"
    );
}

#[tokio::test]
async fn should_spend_a_code_whose_connection_ended_after_approval() {
    let ports = FakePorts::default();
    let user = UserId::new();
    sign_in(&ports, refreshing_document(), user, &[], false).await;
    AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        MetadataDocumentClients::new(enabled(), refreshing_document()),
        AuraResourcePolicy::production(),
    )
    .execute(
        &context(Principal::User(user)),
        AuthorizeRequest {
            response_type: OAuthResponseType::Code,
            client_id: client_id().into(),
            redirect_uri: url("http://127.0.0.1:49152/callback"),
            resource: None,
            scope: HashSet::new(),
            offline_access: false,
            state: None,
            code_challenge: OAuthCodeChallenge::from(CHALLENGE),
            code_challenge_method: CodeChallengeMethod::S256,
        },
    )
    .await
    .unwrap();
    let pending = lock(&ports.0).code.clone().unwrap();
    lock(&ports.0).connections.clear();

    let error = exchange(&ports, &pending).await.unwrap_err();

    assert!(matches!(error, OAuthServiceError::ConnectionNotFound));
    assert!(lock(&ports.0).code.is_none(), "the code is spent");
    assert!(lock(&ports.0).families.len() == 1, "no family starts");
}

#[tokio::test]
async fn should_keep_only_the_most_recently_used_refresh_token_families_of_a_connection() {
    let ports = FakePorts::default();
    let user = UserId::new();
    let mut tokens = Vec::new();
    for _ in 0..=oauth_core::refresh_token::MAX_REFRESH_TOKEN_FAMILIES_PER_CONNECTION {
        tokens.push(
            sign_in(&ports, refreshing_document(), user, &[], false)
                .await
                .refresh_token
                .unwrap(),
        );
    }

    assert_eq!(
        oauth_core::refresh_token::MAX_REFRESH_TOKEN_FAMILIES_PER_CONNECTION,
        lock(&ports.0).families.len()
    );
    assert!(matches!(
        refresh(&ports, &tokens[0]).await,
        Err(OAuthServiceError::RefreshTokenInvalid)
    ));
    assert!(refresh(&ports, tokens.last().unwrap()).await.is_ok());
}

#[tokio::test]
async fn should_disconnect_a_registered_client_by_revoking_its_tokens_for_the_user() {
    let ports = FakePorts::default();
    let user = UserId::new();
    let client_id = OAuthClientId::new();

    disconnect_handler(&ports)
        .execute(
            &context(Principal::User(user)),
            DisconnectOAuthClientCommand {
                user_id: user,
                client_id: client_id.into(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        vec![(user, OAuthTokenHolder::RegisteredClient(client_id))],
        lock(&ports.0).revoked_holders
    );
}

#[tokio::test]
async fn should_require_the_users_own_first_party_session_to_disconnect() {
    let ports = FakePorts::default();
    let user = UserId::new();
    sign_in(&ports, refreshing_document(), user, &[], false).await;

    for (principal, expected) in [
        (
            Principal::DelegatedUser {
                user_id: user,
                capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
            },
            "delegated",
        ),
        (Principal::User(UserId::new()), "forbidden"),
        (Principal::System, "forbidden"),
        (Principal::Anonymous, "anonymous"),
    ] {
        let error = disconnect_handler(&ports)
            .execute(
                &context(principal),
                DisconnectOAuthClientCommand {
                    user_id: user,
                    client_id: client_id().into(),
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (OAuthServiceError::DelegatedDisconnectForbidden, "delegated")
                    | (OAuthServiceError::Forbidden, "forbidden")
                    | (OAuthServiceError::AuthenticatedActorRequired, "anonymous")
            ),
            "{expected}: {error:?}"
        );
    }
    assert_eq!(1, lock(&ports.0).connections.len());
}
