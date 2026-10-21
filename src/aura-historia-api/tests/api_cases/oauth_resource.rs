use super::*;

const MCP_RESOURCE: &str = "https://mcp.aura-historia.com/v1";
const REST_RESOURCE: &str = "https://api.aura-historia.com";

async fn approval_response(
    admin: &OAuthAdmin,
    credentials: &OAuthClientCredentials,
    resources: &[&str],
) -> reqwest::Response {
    let client_id = credentials.client_id.to_string();
    let mut query = vec![
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", "https://client.example/callback"),
        ("scope", "access-tokens:read"),
        ("state", "resource-state"),
        ("code_challenge", PKCE_CODE_CHALLENGE),
        ("code_challenge_method", "S256"),
    ];
    query.extend(resources.iter().map(|resource| ("resource", *resource)));
    admin
        .client
        .get(format!("{}/v1/oauth/authorize", AURA_API.base_url()))
        .bearer_auth(&admin.cognito_token)
        .query(&query)
        .send()
        .await
        .unwrap()
}

async fn redemption_response(
    admin: &OAuthAdmin,
    credentials: &OAuthClientCredentials,
    code: &str,
    resources: &[&str],
) -> reqwest::Response {
    let client_id = credentials.client_id.to_string();
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", "https://client.example/callback"),
        ("client_id", client_id.as_str()),
        ("client_secret", credentials.client_secret.as_str()),
        ("code_verifier", PKCE_CODE_VERIFIER),
    ];
    form.extend(resources.iter().map(|resource| ("resource", *resource)));
    admin
        .client
        .post(format!("{}/v1/oauth/token", AURA_API.base_url()))
        .form(&form)
        .send()
        .await
        .unwrap()
}

fn authorization_code_from(response: reqwest::Response) -> String {
    assert_eq!(reqwest::StatusCode::FOUND, response.status());
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let redirect = url::Url::parse(location).unwrap();
    assert!(
        redirect
            .query_pairs()
            .any(|(key, value)| key == "state" && value == "resource-state")
    );
    redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_issue_a_normal_scoped_aura_token_from_an_mcp_resource_grant_and_revoke_it() {
    let admin = oauth_admin().await;
    let credentials = create_oauth_client(&admin.client, &admin.cognito_token).await;
    let code =
        authorization_code_from(approval_response(&admin, &credentials, &[MCP_RESOURCE]).await);
    let stored: String = sqlx::query_scalar(
        "SELECT resource FROM oauth_authorization_codes WHERE authorization_code = $1",
    )
    .bind(&code)
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap();
    assert_eq!(MCP_RESOURCE, stored);
    let response = redemption_response(&admin, &credentials, &code, &[MCP_RESOURCE]).await;
    let (status, token) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!("Bearer", token["token_type"]);
    assert_eq!("access-tokens:read", token["scope"]);
    assert!(token.get("expires_in").is_none());
    assert!(token.get("refresh_token").is_none());
    let raw = token["access_token"].as_str().unwrap();
    assert!(RawAccessToken::try_from(raw.to_owned()).is_ok());
    let permitted = admin
        .client
        .get(format!("{}/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(reqwest::StatusCode::OK, permitted.status());
    let forbidden = admin
        .client
        .get(format!("{}/v1/me/account", AURA_API.base_url()))
        .bearer_auth(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(reqwest::StatusCode::FORBIDDEN, forbidden.status());
    let revoked = admin
        .client
        .post(format!("{}/v1/oauth/revoke", AURA_API.base_url()))
        .form(&[
            ("client_id", credentials.client_id.to_string()),
            ("client_secret", credentials.client_secret),
            ("token", raw.to_owned()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(reqwest::StatusCode::OK, revoked.status());
    let rejected = admin
        .client
        .get(format!("{}/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(reqwest::StatusCode::UNAUTHORIZED, rejected.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_foreign_ambiguous_and_substituted_resource_indicators() {
    let admin = oauth_admin().await;
    let credentials = create_oauth_client(&admin.client, &admin.cognito_token).await;
    for resources in [
        vec!["https://foreign.example"],
        vec!["https://mcp.stage.aura-historia.com/v1"],
        vec!["https://mcp.aura-historia.com/v1#fragment"],
        vec![""],
        vec!["https://foreign.example", MCP_RESOURCE],
        vec![MCP_RESOURCE, "https://foreign.example"],
    ] {
        let (status, error) =
            json_response(approval_response(&admin, &credentials, &resources).await).await;
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, status, "{resources:?}");
        assert_eq!("OAUTH_INVALID_RESOURCE", error["error"]);
    }
    let code =
        authorization_code_from(approval_response(&admin, &credentials, &[MCP_RESOURCE]).await);
    for resources in [
        vec!["https://foreign.example"],
        vec!["https://api.stage.aura-historia.com"],
        vec![MCP_RESOURCE, "https://foreign.example"],
    ] {
        let (status, error) =
            json_response(redemption_response(&admin, &credentials, &code, &resources).await).await;
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, status, "{resources:?}");
        assert_eq!("OAUTH_INVALID_RESOURCE", error["error"]);
    }
    let (status, error) =
        json_response(redemption_response(&admin, &credentials, &code, &[REST_RESOURCE]).await)
            .await;
    assert_eq!(reqwest::StatusCode::BAD_REQUEST, status);
    assert_eq!("OAUTH_INVALID_RESOURCE", error["error"]);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM oauth_authorization_codes WHERE authorization_code = $1",
    )
    .bind(&code)
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap();
    assert_eq!(0, count);
}
