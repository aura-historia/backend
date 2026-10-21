use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::Url;
use user_core::user_id::UserId;

pub const MCP_RESOURCE: &str = "https://mcp.aura-historia.com/v1";
pub const CALLBACK: &str = "https://generic-client.example/callback";

/// A confidential client registration, shared across independent user authorization attempts.
pub struct GenericClient {
    pub http: reqwest::Client,
    pub id: String,
    secret: String,
}

pub struct AuthorizationAttempt {
    pub browser_url: Url,
    pub query: BTreeMap<String, String>,
    verifier: String,
}

impl GenericClient {
    pub async fn registered() -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();
        let admin = api_support::seed_user("ADMIN").await;
        let response = http
            .post(format!("{}/v1/admin/oauth-clients", AURA_API.base_url()))
            .bearer_auth(api_support::cognito_access_token_for_test_user(admin))
            .json(&json!({
                "client_name":"Generic MCP acceptance client",
                "tos_uri":"https://generic-client.example/terms",
                "policy_uri":"https://generic-client.example/privacy",
                "client_uri":"https://generic-client.example",
                "logo_uri":"https://generic-client.example/logo.png",
                "redirect_uris":[CALLBACK],
                "scope":["users:read","users:write","access-tokens:write"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::CREATED, response.status());
        let body: Value = response.json().await.unwrap();
        Self {
            http,
            id: body["client_id"].as_str().unwrap().into(),
            secret: body["client_secret"].as_str().unwrap().into(),
        }
    }

    pub async fn attempt(&self, scopes: &str) -> AuthorizationAttempt {
        let metadata = self
            .http
            .get(format!(
                "{}/.well-known/oauth-authorization-server",
                AURA_API.base_url()
            ))
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&metadata);
        assert_eq!(StatusCode::OK, metadata.status());
        let metadata: Value = metadata.json().await.unwrap();
        assert_eq!(
            json!(["S256"]),
            metadata["code_challenge_methods_supported"]
        );
        assert_eq!(
            json!(["client_secret_post"]),
            metadata["token_endpoint_auth_methods_supported"]
        );
        assert_eq!(
            "https://api.aura-historia.com/v1/oauth/token",
            metadata["token_endpoint"]
        );
        assert!(metadata.get("registration_endpoint").is_none());
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let query = BTreeMap::from([
            ("response_type".into(), "code".into()),
            ("client_id".into(), self.id.clone()),
            ("redirect_uri".into(), CALLBACK.into()),
            ("scope".into(), scopes.into()),
            ("state".into(), uuid::Uuid::new_v4().to_string()),
            ("code_challenge".into(), challenge),
            ("code_challenge_method".into(), "S256".into()),
            ("resource".into(), MCP_RESOURCE.into()),
        ]);
        let mut browser_url =
            Url::parse(metadata["authorization_endpoint"].as_str().unwrap()).unwrap();
        assert_eq!(
            "https://aura-historia.com/en/oauth/authorize",
            browser_url.as_str()
        );
        browser_url.query_pairs_mut().extend_pairs(&query);
        assert!(!browser_url.as_str().contains(&self.secret));
        AuthorizationAttempt {
            browser_url,
            query,
            verifier,
        }
    }

    pub async fn approve(&self, attempt: &AuthorizationAttempt, bearer: &str) -> reqwest::Response {
        // The webapp owns rendering/login and forwarding these parameters. Exercise the real
        // backend approval boundary with exactly the advertised handoff parameters.
        assert_eq!(
            attempt.query,
            attempt
                .browser_url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        );
        self.http
            .get(format!("{}/v1/oauth/authorize", AURA_API.base_url()))
            .bearer_auth(bearer)
            .query(&attempt.query)
            .send()
            .await
            .unwrap()
    }

    pub async fn code(&self, attempt: &AuthorizationAttempt, user: UserId) -> String {
        let response = self
            .approve(
                attempt,
                &api_support::cognito_access_token_for_test_user(user),
            )
            .await;
        assert_credentials_headers(&response);
        assert_eq!(StatusCode::FOUND, response.status());
        let code = attempt
            .callback(response.headers()["location"].to_str().unwrap())
            .unwrap()
            .unwrap();
        let resource: String = sqlx::query_scalar(
            "SELECT resource FROM oauth_authorization_codes WHERE authorization_code=$1",
        )
        .bind(&code)
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap();
        assert_eq!(MCP_RESOURCE, resource);
        code
    }

    pub fn exchange_form(
        &self,
        attempt: &AuthorizationAttempt,
        code: &str,
    ) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("grant_type".into(), "authorization_code".into()),
            ("client_id".into(), self.id.clone()),
            ("client_secret".into(), self.secret.clone()),
            ("code".into(), code.into()),
            ("redirect_uri".into(), CALLBACK.into()),
            ("code_verifier".into(), attempt.verifier.clone()),
            ("resource".into(), MCP_RESOURCE.into()),
        ])
    }

    pub async fn exchange(&self, form: &BTreeMap<String, String>) -> reqwest::Response {
        let response = self
            .http
            .post(format!("{}/v1/oauth/token", AURA_API.base_url()))
            .form(form)
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&response);
        response
    }

    pub async fn link(&self, user: UserId, scopes: &str) -> Value {
        let attempt = self.attempt(scopes).await;
        let code = self.code(&attempt, user).await;
        let response = self.exchange(&self.exchange_form(&attempt, &code)).await;
        assert_eq!(StatusCode::OK, response.status());
        let body: Value = response.json().await.unwrap();
        assert_eq!("Bearer", body["token_type"]);
        assert!(body.get("expires_in").is_none());
        assert!(body.get("refresh_token").is_none());
        assert_eq!(
            scopes
                .split_whitespace()
                .collect::<std::collections::BTreeSet<_>>(),
            body["scope"].as_str().unwrap().split_whitespace().collect()
        );
        body
    }

    pub async fn revoke(&self, token: &str) {
        let response = self
            .http
            .post(format!("{}/v1/oauth/revoke", AURA_API.base_url()))
            .form(&[
                ("client_id", self.id.as_str()),
                ("client_secret", self.secret.as_str()),
                ("token", token),
            ])
            .send()
            .await
            .unwrap();
        assert_credentials_headers(&response);
        assert_eq!(StatusCode::OK, response.status());
    }

    pub async fn rest(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> reqwest::Response {
        let request = self
            .http
            .request(method, format!("{}{path}", AURA_API.base_url()))
            .bearer_auth(token);
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        request.send().await.unwrap()
    }
}

impl AuthorizationAttempt {
    /// Callback handling belongs to the generic client, including user denial and state checks.
    pub fn callback(&self, location: &str) -> Result<Option<String>, &'static str> {
        let callback = Url::parse(location).map_err(|_| "invalid callback")?;
        let mut base = callback.clone();
        base.set_query(None);
        if base.as_str() != CALLBACK || callback.fragment().is_some() {
            return Err("wrong callback");
        }
        let pairs = callback.query_pairs().collect::<Vec<_>>();
        let mut query = BTreeMap::new();
        for (key, value) in pairs {
            if query.insert(key.into_owned(), value.into_owned()).is_some() {
                return Err("duplicate callback parameter");
            }
        }
        if query.get("state") != self.query.get("state") {
            return Err("wrong state");
        }
        match (query.get("error").map(String::as_str), query.get("code")) {
            (Some("access_denied"), None) => Ok(None),
            (None, Some(code)) if !code.is_empty() => Ok(Some(code.clone())),
            _ => Err("invalid authorization response"),
        }
    }
}

pub fn assert_credentials_headers(response: &reqwest::Response) {
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert_eq!("no-cache", response.headers()["pragma"]);
}
