use super::*;
use oauth_core::metadata_document_client::DeniedMetadataDocumentHosts;
use oauth_service::use_cases::{
    GetMetadataDocumentConsentClientHandler, GetMetadataDocumentConsentClientRequest,
    GetMetadataDocumentConsentClientUseCase,
};

const CLIENT_ID: &str = "https://agent.example/oauth/client.json";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

fn client_id() -> MetadataDocumentClientId {
    MetadataDocumentClientId::try_from(CLIENT_ID).unwrap()
}

fn claims() -> ClientMetadataDocumentClaims {
    ClientMetadataDocumentClaims {
        client_id: Some(CLIENT_ID.to_owned()),
        client_name: Some("Example Agent".to_owned()),
        redirect_uris: Some(vec![
            "http://127.0.0.1/callback".to_owned(),
            "http://localhost/callback".to_owned(),
        ]),
        token_endpoint_auth_method: Some("none".to_owned()),
        grant_types: Some(vec!["authorization_code".to_owned()]),
        response_types: Some(vec!["code".to_owned()]),
        client_uri: Some("https://agent.example".to_owned()),
        logo_uri: None,
        declares_client_secret: false,
    }
}

/// Serves one document, or fails as unavailable, and counts loads.
#[derive(Clone)]
struct FakeDocuments {
    claims: Option<ClientMetadataDocumentClaims>,
    loads: Arc<Mutex<usize>>,
}

impl FakeDocuments {
    fn serving(claims: ClientMetadataDocumentClaims) -> Self {
        Self {
            claims: Some(claims),
            loads: Arc::default(),
        }
    }

    fn unavailable() -> Self {
        Self {
            claims: None,
            loads: Arc::default(),
        }
    }

    fn loads(&self) -> usize {
        *lock(&self.loads)
    }
}

#[async_trait::async_trait]
impl ClientMetadataDocumentSource for FakeDocuments {
    async fn load(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<ClientMetadataDocumentClaims, ClientMetadataDocumentLoadError> {
        *lock(&self.loads) += 1;
        assert_eq!(CLIENT_ID, client_id.as_str());
        self.claims
            .clone()
            .ok_or_else(|| ClientMetadataDocumentLoadError::Unavailable {
                source: application::error::box_error(std::io::Error::other("timed out")),
            })
    }
}

fn enabled() -> MetadataDocumentClientPolicy {
    MetadataDocumentClientPolicy::Enabled {
        denied_hosts: DeniedMetadataDocumentHosts::default(),
    }
}

fn authorize_handler(
    ports: &FakePorts,
    policy: MetadataDocumentClientPolicy,
    documents: FakeDocuments,
) -> impl AuthorizeUseCase {
    AuthorizeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        MetadataDocumentClients::new(policy, documents),
        AuraResourcePolicy::production(),
    )
}

fn token_handler(
    ports: &FakePorts,
    policy: MetadataDocumentClientPolicy,
) -> impl TokenByAuthorizationCodeUseCase {
    TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        policy,
        AuraResourcePolicy::production(),
    )
}

fn request(redirect_uri: &str, scope: HashSet<Scope>) -> AuthorizeRequest {
    AuthorizeRequest {
        response_type: OAuthResponseType::Code,
        client_id: client_id().into(),
        redirect_uri: url(redirect_uri),
        resource: Some("https://mcp.aura-historia.com/v1".to_owned()),
        scope,
        state: Some(OAuthState::from("state-1")),
        code_challenge: OAuthCodeChallenge::from("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        code_challenge_method: CodeChallengeMethod::S256,
    }
}

fn token_request(
    code: &AuthorizationCode,
    client: OAuthTokenClientAuthentication,
) -> TokenByAuthorizationCodeRequest {
    TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: code.redirect_uri().clone(),
        resource: None,
        client,
        code_verifier: OAuthCodeVerifier::from(VERIFIER),
    }
}

fn public_client() -> OAuthTokenClientAuthentication {
    OAuthTokenClientAuthentication::None {
        client_id: client_id(),
    }
}

async fn approve(ports: &FakePorts) -> (AuthorizeResponse, AuthorizationCode) {
    let response = authorize_handler(ports, enabled(), FakeDocuments::serving(claims()))
        .execute(
            &first_party_context(),
            request(
                "http://127.0.0.1:49152/callback",
                HashSet::from([Scope::UsersRead, Scope::WatchlistWrite]),
            ),
        )
        .await
        .unwrap();
    let code = lock(&ports.0).code.clone().unwrap();
    (response, code)
}

#[tokio::test]
async fn should_issue_an_expiring_token_without_exchange_code_to_a_metadata_document_client() {
    let ports = FakePorts::default();
    let (response, code) = approve(&ports).await;

    let redirect = url(&response.redirect_to);
    assert_eq!("127.0.0.1", redirect.host_str().unwrap());
    assert_eq!(Some(49152), redirect.port());
    assert_eq!(
        vec![
            ("code".to_owned(), code.code().to_string()),
            ("state".to_owned(), "state-1".to_owned()),
            ("iss".to_owned(), "https://api.aura-historia.com".to_owned()),
        ],
        redirect.query_pairs().into_owned().collect::<Vec<_>>()
    );
    assert_eq!(&OAuthClientIdentifier::from(client_id()), code.client_id());

    let issued_at = OffsetDateTime::now_utc();
    let token = token_handler(&ports, enabled())
        .execute(token_request(&code, public_client()))
        .await
        .unwrap();

    let expires = token
        .expires
        .expect("public clients receive expiring tokens");
    assert!(expires > issued_at + time::Duration::minutes(59));
    assert!(expires <= OffsetDateTime::now_utc() + time::Duration::hours(1));
    assert_eq!(None, token.third_party_exchange_code);
    assert_eq!(
        HashSet::from([Scope::UsersRead, Scope::WatchlistWrite]),
        token.scopes
    );
    let state = lock(&ports.0);
    let issued = state.issued.clone().unwrap();
    assert_eq!(
        &AccessTokenOrigin::OAuthMetadataDocument {
            client_id: client_id()
        },
        issued.origin()
    );
    assert_eq!(Some(expires), issued.expires());
    assert!(
        issued
            .name()
            .as_ref()
            .starts_with("agent.example (OAuth-Client ")
    );
    assert!(state.exchange.is_none());
    assert!(state.code.is_none());
}

#[tokio::test]
async fn should_add_the_issuer_to_registered_client_redirects() {
    let ports = FakePorts::default();
    let client = client_with_secret(&RawOAuthClientSecret::new());
    lock(&ports.0).client = Some(client.clone());

    let response = authorize_handler(&ports, enabled(), FakeDocuments::unavailable())
        .execute(
            &first_party_context(),
            authorize_request(
                client.client_id(),
                HashSet::from([Scope::ProductListingsWrite]),
            ),
        )
        .await
        .unwrap();

    let redirect = url(&response.redirect_to);
    assert_eq!(
        Some("https://api.aura-historia.com".to_owned()),
        redirect
            .query_pairs()
            .find(|(key, _)| key == "iss")
            .map(|(_, value)| value.into_owned())
    );
}

#[tokio::test]
async fn should_reject_an_unavailable_or_invalid_document_before_opening_a_transaction() {
    for documents in [
        FakeDocuments::unavailable(),
        FakeDocuments::serving(ClientMetadataDocumentClaims {
            client_id: Some("https://agent.example/other.json".to_owned()),
            ..claims()
        }),
        FakeDocuments::serving(ClientMetadataDocumentClaims {
            token_endpoint_auth_method: Some("client_secret_post".to_owned()),
            ..claims()
        }),
    ] {
        let ports = FakePorts::default();
        let error = authorize_handler(&ports, enabled(), documents.clone())
            .execute(
                &first_party_context(),
                request("http://127.0.0.1:49152/callback", HashSet::new()),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            OAuthServiceError::InvalidMetadataDocumentClient { .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(1, documents.loads());
        assert_eq!(0, lock(&ports.0).transaction_begins);
    }
}

#[tokio::test]
async fn should_reject_unlisted_callbacks_and_scopes_beyond_the_ordinary_user_vocabulary() {
    for (redirect_uri, scope, expected) in [
        ("http://[::1]:49152/callback", HashSet::new(), "redirect"),
        ("https://agent.example/callback", HashSet::new(), "redirect"),
        (
            "http://127.0.0.1:49152/callback",
            HashSet::from([Scope::AccessTokensWrite]),
            "scope",
        ),
        (
            "http://127.0.0.1:49152/callback",
            HashSet::from([Scope::ProductListingsWrite, Scope::UsersRead]),
            "scope",
        ),
    ] {
        let ports = FakePorts::default();
        let error = authorize_handler(&ports, enabled(), FakeDocuments::serving(claims()))
            .execute(&first_party_context(), request(redirect_uri, scope))
            .await
            .unwrap_err();

        match expected {
            "redirect" => assert!(matches!(error, OAuthServiceError::InvalidRedirectUri)),
            _ => assert!(matches!(error, OAuthServiceError::InvalidScope)),
        }
        assert!(lock(&ports.0).code.is_none());
    }
}

#[tokio::test]
async fn should_refuse_metadata_document_clients_that_the_policy_does_not_admit() {
    let denied = MetadataDocumentClientPolicy::Enabled {
        denied_hosts: DeniedMetadataDocumentHosts::parse(["example"]).unwrap(),
    };
    for policy in [MetadataDocumentClientPolicy::Disabled, denied] {
        let ports = FakePorts::default();
        let documents = FakeDocuments::serving(claims());
        let error = authorize_handler(&ports, policy.clone(), documents.clone())
            .execute(
                &first_party_context(),
                request("http://127.0.0.1:49152/callback", HashSet::new()),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OAuthServiceError::InvalidMetadataDocumentClient { .. }
        ));
        assert_eq!(0, documents.loads());

        // A code approved earlier cannot be exchanged once the policy refuses the client.
        let approved = FakePorts::default();
        let (_, code) = approve(&approved).await;
        let error = token_handler(&approved, policy)
            .execute(token_request(&code, public_client()))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OAuthServiceError::InvalidMetadataDocumentClient { .. }
        ));
        assert!(
            lock(&approved.0).code.is_some(),
            "the code stays unconsumed"
        );
        assert!(lock(&approved.0).issued.is_none());
    }
}

#[tokio::test]
async fn should_never_exchange_a_code_across_client_kinds_or_clients() {
    // A metadata document client's code presented by a registered client.
    let ports = FakePorts::default();
    let (_, code) = approve(&ports).await;
    let secret = RawOAuthClientSecret::new();
    let registered_client = client_with_secret(&secret);
    lock(&ports.0).client = Some(registered_client.clone());
    let error = token_handler(&ports, enabled())
        .execute(token_request(
            &code,
            OAuthTokenClientAuthentication::ClientSecretPost {
                client_id: registered_client.client_id(),
                client_secret: secret.clone(),
            },
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        OAuthServiceError::AuthorizationCodeClientMismatch
    ));

    // Another metadata document client.
    let ports = FakePorts::default();
    let (_, code) = approve(&ports).await;
    let error = token_handler(&ports, enabled())
        .execute(token_request(
            &code,
            OAuthTokenClientAuthentication::None {
                client_id: MetadataDocumentClientId::try_from(
                    "https://agent.example/oauth/other.json",
                )
                .unwrap(),
            },
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        OAuthServiceError::AuthorizationCodeClientMismatch
    ));
    assert!(
        lock(&ports.0).code.is_some(),
        "a public client cannot burn another client's code"
    );

    // A registered client's code presented without a secret under a URL client ID.
    let ports = FakePorts::default();
    let code = authorization_code(
        registered_client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    lock(&ports.0).client = Some(registered_client);
    lock(&ports.0).code = Some(code.clone());
    let error = token_handler(&ports, enabled())
        .execute(token_request(&code, public_client()))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        OAuthServiceError::AuthorizationCodeClientMismatch
    ));
    assert!(lock(&ports.0).issued.is_none());
    assert!(lock(&ports.0).code.is_some());
}

#[tokio::test]
async fn should_keep_pkce_mandatory_for_metadata_document_clients() {
    let ports = FakePorts::default();
    let (_, code) = approve(&ports).await;
    let mut request = token_request(&code, public_client());
    request.code_verifier = OAuthCodeVerifier::from("x".repeat(43).as_str());

    let error = token_handler(&ports, enabled())
        .execute(request)
        .await
        .unwrap_err();

    assert!(matches!(error, OAuthServiceError::InvalidCodeVerifier));
    assert!(lock(&ports.0).issued.is_none());
}

fn consent_handler(documents: FakeDocuments) -> impl GetMetadataDocumentConsentClientUseCase {
    GetMetadataDocumentConsentClientHandler::new(MetadataDocumentClients::new(enabled(), documents))
}

fn consent_request(redirect_uri: &str) -> GetMetadataDocumentConsentClientRequest {
    GetMetadataDocumentConsentClientRequest {
        client_id: client_id(),
        redirect_uri: url(redirect_uri),
    }
}

#[tokio::test]
async fn should_describe_the_verified_hosts_of_a_metadata_document_client_for_consent() {
    let view = consent_handler(FakeDocuments::serving(claims()))
        .execute(
            &first_party_context(),
            consent_request("http://localhost:33418/callback"),
        )
        .await
        .unwrap();

    assert_eq!(client_id(), view.client_id);
    assert_eq!("Example Agent", view.name.as_ref());
    assert_eq!(Some(url("https://agent.example")), view.client_uri);
    assert_eq!(None, view.logo_uri);
    assert_eq!("agent.example", view.client_host);
    assert_eq!("localhost", view.redirect_uri_host);
    assert!(view.loopback_redirect_uris_only);
    assert_eq!(
        oauth_core::metadata_document_client::metadata_document_client_scopes(),
        view.scopes
    );

    let hosted = consent_handler(FakeDocuments::serving(ClientMetadataDocumentClaims {
        redirect_uris: Some(vec!["https://callback.example/oauth".to_owned()]),
        ..claims()
    }))
    .execute(
        &first_party_context(),
        consent_request("https://callback.example/oauth"),
    )
    .await
    .unwrap();
    assert_eq!("callback.example", hosted.redirect_uri_host);
    assert!(!hosted.loopback_redirect_uris_only);
}

#[tokio::test]
async fn should_refuse_consent_metadata_for_unlisted_callbacks_and_non_first_party_actors() {
    let error = consent_handler(FakeDocuments::serving(claims()))
        .execute(
            &first_party_context(),
            consent_request("https://attacker.example/callback"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, OAuthServiceError::InvalidRedirectUri));

    for (context, anonymous) in [
        (context(Principal::Anonymous), true),
        (delegated_admin_context(), false),
    ] {
        let documents = FakeDocuments::serving(claims());
        let error = consent_handler(documents.clone())
            .execute(&context, consent_request("http://localhost:1/callback"))
            .await
            .unwrap_err();
        if anonymous {
            assert!(matches!(
                error,
                OAuthServiceError::AuthenticatedActorRequired
            ));
        } else {
            assert!(matches!(error, OAuthServiceError::Forbidden));
        }
        assert_eq!(0, documents.loads());
    }
}
