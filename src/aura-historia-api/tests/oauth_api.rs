mod api_support;

use api_support::{json_response, seed_access_token_for, seed_user};

use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_run_oauth_rest_flow() {
    let user_id = seed_user("USER").await;
    let admin_token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|error| panic!("failed to build HTTP client: {error}"));

    let response = client
        .post(format!("{}/api/v1/oauth/clients", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&serde_json::json!({
            "client_name": "Acceptance OAuth Client",
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback"],
            "scope": ["products:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create OAuth client: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status);
    let client_id = body["client_id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing client_id"))
        .to_owned();
    let client_secret = body["client_secret"]
        .as_str()
        .unwrap_or_else(|| panic!("missing client_secret"))
        .to_owned();

    let response = client
        .get(format!("{}/api/v1/oauth/clients", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list OAuth clients: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body.as_array().map(Vec::len));

    let response = client
        .get(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            client_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());

    let response = client
        .patch(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            client_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&serde_json::json!({ "client_name": "Acceptance OAuth Client Updated" }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update OAuth client: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!("Acceptance OAuth Client Updated"),
        body["client_name"]
    );

    let response = client
        .get(format!("{}/api/v1/oauth/authorize", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "https://client.example/callback"),
            ("scope", "products:write"),
            ("state", "acceptance-state"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to authorize OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::FOUND, response.status());
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("missing redirect location"));
    let redirect =
        url::Url::parse(location).unwrap_or_else(|error| panic!("invalid redirect URL: {error}"));
    let code = redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
        .unwrap_or_else(|| panic!("missing authorization code"));

    let response = client
        .post(format!("{}/api/v1/oauth/token", AURA_API.base_url()))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "https://client.example/callback"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to exchange OAuth token: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    let access_token = body["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing access token"))
        .to_owned();
    let third_party_code = body["third_party_exchange_code"]
        .as_str()
        .unwrap_or_else(|| panic!("missing third party code"))
        .to_owned();

    let response = client
        .post(format!("{}/api/v1/oauth/introspect", AURA_API.base_url()))
        .form(&[
            ("token", access_token.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to introspect token: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(true), body["active"]);

    let response = client
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            AURA_API.base_url(),
            third_party_code
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to exchange third party code: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());

    let response = client
        .post(format!("{}/api/v1/oauth/revoke", AURA_API.base_url()))
        .form(&[
            ("token", access_token.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke token: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());

    let response = client
        .delete(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            client_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}
