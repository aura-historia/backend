//! A public agent signs in with only a Client ID Metadata Document: no registration, no
//! secret, a loopback callback on an ephemeral port, then an MCP call with the token.
use super::*;
use crate::METADATA_DOCUMENT_API;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use client::assert_credentials_headers;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::Url;
use user_core::user_id::UserId;

const CLIENT_ID: &str = "https://agent.test/oauth/client.json";
const CALLBACK: &str = "http://127.0.0.1:49152/callback";
const ISSUER: &str = "https://api.aura-historia.com";

/// A public client: it holds a PKCE verifier and nothing else.
struct PublicAgent {
    http: reqwest::Client,
    client_id: String,
    verifier: String,
}

impl PublicAgent {
    fn new(client_id: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap(),
            client_id: client_id.to_owned(),
            verifier: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    }

    fn authorization_query(&self, redirect_uri: &str, scope: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("response_type".into(), "code".into()),
            ("client_id".into(), self.client_id.clone()),
            ("redirect_uri".into(), redirect_uri.into()),
            ("scope".into(), scope.into()),
            ("state".into(), "agent-state".into()),
            (
                "code_challenge".into(),
                URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes())),
            ),
            ("code_challenge_method".into(), "S256".into()),
            ("resource".into(), MCP_RESOURCE.into()),
        ])
    }

    async fn consent(&self, user: UserId, redirect_uri: &str) -> reqwest::Response {
        let response = self
            .http
            .get(format!(
                "{}/v1/oauth/clients",
                METADATA_DOCUMENT_API.base_url()
            ))
            .bearer_auth(api_support::cognito_access_token_for_test_user(user))
            .query(&[
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&response);
        response
    }

    async fn approve(&self, user: UserId, redirect_uri: &str, scope: &str) -> reqwest::Response {
        let response = self
            .http
            .get(format!(
                "{}/v1/oauth/authorize",
                METADATA_DOCUMENT_API.base_url()
            ))
            .bearer_auth(api_support::cognito_access_token_for_test_user(user))
            .query(&self.authorization_query(redirect_uri, scope))
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&response);
        response
    }

    async fn exchange(&self, code: &str, extra: &[(&str, &str)]) -> reqwest::Response {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("client_id", self.client_id.as_str()),
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("code_verifier", self.verifier.as_str()),
            ("resource", MCP_RESOURCE),
        ];
        form.extend_from_slice(extra);
        let response = self
            .http
            .post(format!(
                "{}/v1/oauth/token",
                METADATA_DOCUMENT_API.base_url()
            ))
            .form(&form)
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&response);
        response
    }
}

/// The authorization response parameters, after checking it returns to the agent's callback.
fn authorization_response(location: &str) -> BTreeMap<String, String> {
    let callback = Url::parse(location).unwrap();
    assert_eq!("http", callback.scheme());
    assert_eq!(Some("127.0.0.1"), callback.host_str());
    assert_eq!(Some(49152), callback.port());
    assert_eq!("/callback", callback.path());
    callback.query_pairs().into_owned().collect()
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &METADATA_DOCUMENT_API], isolated = true)]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_sign_in_a_metadata_document_client_and_call_mcp_with_its_token() {
    let agent = PublicAgent::new(CLIENT_ID);
    let user = api_support::seed_user("USER").await;
    let metadata: Value = agent
        .http
        .get(format!(
            "{}/.well-known/oauth-authorization-server",
            METADATA_DOCUMENT_API.base_url()
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        json!(true),
        metadata["client_id_metadata_document_supported"]
    );
    assert_eq!(
        json!(["client_secret_post", "none"]),
        metadata["token_endpoint_auth_methods_supported"]
    );
    assert_eq!(
        json!(true),
        metadata["authorization_response_iss_parameter_supported"]
    );

    let consent = agent.consent(user, CALLBACK).await;
    assert_eq!(StatusCode::OK, consent.status());
    let consent: Value = consent.json().await.unwrap();
    assert_eq!(
        json!({
            "client_id": CLIENT_ID,
            "client_name": "Acceptance Agent",
            "client_uri": "https://agent.test/",
            "client_host": "agent.test",
            "redirect_uri_host": "127.0.0.1",
            "loopback_redirect_uris_only": true,
            "scope": [
                "notifications:read", "notifications:write", "search-filters:read",
                "search-filters:write", "users:read", "users:write", "watchlist:read",
                "watchlist:write"
            ],
        }),
        consent
    );

    let approval = agent.approve(user, CALLBACK, "users:read").await;
    assert_eq!(StatusCode::FOUND, approval.status());
    let response = authorization_response(approval.headers()["location"].to_str().unwrap());
    assert_eq!(Some(&ISSUER.to_owned()), response.get("iss"));
    assert_eq!(Some(&"agent-state".to_owned()), response.get("state"));
    let code = response["code"].clone();
    let (registered, metadata_document): (Option<uuid::Uuid>, Option<String>) = sqlx::query_as(
        "SELECT client_id, metadata_document_client_id FROM oauth_authorization_codes \
         WHERE authorization_code = $1",
    )
    .bind(&code)
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap();
    assert_eq!(
        (None, Some(CLIENT_ID.to_owned())),
        (registered, metadata_document)
    );

    let exchange = agent.exchange(&code, &[]).await;
    assert_eq!(StatusCode::OK, exchange.status());
    let token: Value = exchange.json().await.unwrap();
    assert_eq!("Bearer", token["token_type"]);
    assert_eq!("users:read", token["scope"]);
    let expires_in = token["expires_in"].as_i64().unwrap();
    assert!((3_500..=3_600).contains(&expires_in), "{expires_in}");
    assert!(token.get("refresh_token").is_none());
    assert!(token.get("third_party_exchange_code").is_none());
    let access_token = token["access_token"].as_str().unwrap();
    let (origin, oauth_client_id, metadata_document_client_id): (
        String,
        Option<uuid::Uuid>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT origin, oauth_client_id, oauth_metadata_document_client_id FROM access_tokens \
         WHERE user_id = $1",
    )
    .bind(user.into_uuid())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap();
    assert_eq!(
        ("OAUTH_METADATA_DOCUMENT", None, Some(CLIENT_ID)),
        (
            origin.as_str(),
            oauth_client_id,
            metadata_document_client_id.as_deref()
        )
    );

    let mcp = McpServer::start().await;
    let account = successful_result(
        mcp.call("getUserAccount", json!({}), Some(access_token))
            .await,
    )
    .await;
    assert_eq!(json!(user), account["userId"]);

    let replay = agent.exchange(&code, &[]).await;
    assert_eq!(StatusCode::BAD_REQUEST, replay.status());
    assert_eq!(
        json!({"error": "invalid_grant"}),
        replay.json::<Value>().await.unwrap()
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &METADATA_DOCUMENT_API], isolated = true)]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_refuse_unusable_documents_unlisted_callbacks_and_secrets() {
    let user = api_support::seed_user("USER").await;
    // Mismatched client_id, redirect response, oversized document, timeout, denylisted host,
    // and a host resolving to a private address under the production outbound policy.
    for client_id in [
        "https://agent.test/oauth/mismatch.json",
        "https://agent.test/oauth/redirect.json",
        "https://agent.test/oauth/oversized.json",
        "https://agent.test/oauth/slow.json",
        "https://agent.test/oauth/missing.json",
        "https://denied.test/oauth/client.json",
        "https://localhost/oauth/client.json",
    ] {
        let agent = PublicAgent::new(client_id);
        let consent = agent.consent(user, CALLBACK).await;
        assert_eq!(StatusCode::BAD_REQUEST, consent.status(), "{client_id}");
        assert_eq!(
            "OAUTH_INVALID_CLIENT",
            consent.json::<Value>().await.unwrap()["error"],
            "{client_id}"
        );
        let approval = agent.approve(user, CALLBACK, "users:read").await;
        assert_eq!(StatusCode::BAD_REQUEST, approval.status(), "{client_id}");
        assert!(approval.headers().get("location").is_none());
        assert_eq!(
            json!({"error": "invalid_client"}),
            approval.json::<Value>().await.unwrap(),
            "{client_id}"
        );
    }

    let agent = PublicAgent::new(CLIENT_ID);
    for callback in [
        "http://localhost.attacker.test:49152/callback",
        "https://agent.test/callback",
        "http://127.0.0.1:49152/other",
    ] {
        let consent = agent.consent(user, callback).await;
        assert_eq!(StatusCode::BAD_REQUEST, consent.status(), "{callback}");
        assert_eq!(
            "OAUTH_INVALID_REDIRECT_URI",
            consent.json::<Value>().await.unwrap()["error"]
        );
        let approval = agent.approve(user, callback, "users:read").await;
        assert_eq!(StatusCode::BAD_REQUEST, approval.status(), "{callback}");
        assert!(approval.headers().get("location").is_none());
    }
    let beyond_ceiling = agent
        .approve(user, CALLBACK, "users:read access-tokens:write")
        .await;
    assert_eq!(
        json!({"error": "invalid_scope"}),
        beyond_ceiling.json::<Value>().await.unwrap()
    );
    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_authorization_codes")
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap();
    assert_eq!(0, codes, "no refused request may issue a code");

    // A public client presents no secret, and only the client a code was issued to may use it.
    let approval = agent.approve(user, CALLBACK, "users:read").await;
    let code =
        authorization_response(approval.headers()["location"].to_str().unwrap())["code"].clone();
    let with_secret = agent
        .exchange(
            &code,
            &[("client_secret", "aurahistoria_oauth_client_secret_abc_def")],
        )
        .await;
    assert_eq!(StatusCode::UNAUTHORIZED, with_secret.status());
    assert_eq!(
        json!({"error": "invalid_client"}),
        with_secret.json::<Value>().await.unwrap()
    );
    let mut impostor = PublicAgent::new("https://agent.test/oauth/other.json");
    impostor.verifier = agent.verifier.clone();
    let swapped = impostor.exchange(&code, &[]).await;
    assert_eq!(StatusCode::BAD_REQUEST, swapped.status());
    assert_eq!(
        json!({"error": "invalid_grant"}),
        swapped.json::<Value>().await.unwrap()
    );
    let consumed = agent.exchange(&code, &[]).await;
    assert_eq!(StatusCode::BAD_REQUEST, consumed.status());
}
