use super::*;
use client::assert_credentials_headers;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_reject_malformed_authorization_and_exact_callback_client_or_resource_mismatches() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    for (field, value, expected) in [
        (
            "redirect_uri",
            "https://generic-client.example/callback/suffix",
            "invalid_request",
        ),
        (
            "redirect_uri",
            "https://generic-client.example/callback?extra=1",
            "invalid_request",
        ),
        ("client_id", "malformed", "invalid_request"),
        ("code_challenge_method", "plain", "invalid_request"),
        ("code_challenge", "malformed", "invalid_request"),
        ("scope", "notifications:read", "invalid_scope"),
        ("resource", "https://foreign.example", "invalid_target"),
        (
            "resource",
            "https://mcp.stage.aura-historia.com/v1",
            "invalid_target",
        ),
        (
            "resource",
            "https://mcp.aura-historia.com/v1#fragment",
            "invalid_target",
        ),
    ] {
        let mut attempt = client.attempt("users:read").await;
        attempt.query.insert(field.into(), value.into());
        attempt.browser_url.set_query(None);
        attempt
            .browser_url
            .query_pairs_mut()
            .extend_pairs(&attempt.query);
        let response = client
            .approve(
                &attempt,
                &api_support::cognito_access_token_for_test_user(user),
            )
            .await;
        assert!(response.headers().get("location").is_none());
        assert_oauth_error(response, StatusCode::BAD_REQUEST, expected).await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_reject_wrong_client_callback_verifier_code_and_resource_during_exchange() {
    let client = GenericClient::registered().await;
    let other = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    for (field, value, status, expected) in [
        (
            "redirect_uri",
            "https://generic-client.example/callback?extra=1".into(),
            StatusCode::BAD_REQUEST,
            "invalid_grant",
        ),
        (
            "code_verifier",
            "x".repeat(43),
            StatusCode::BAD_REQUEST,
            "invalid_grant",
        ),
        (
            "code_verifier",
            "short".into(),
            StatusCode::BAD_REQUEST,
            "invalid_grant",
        ),
        (
            "client_id",
            "malformed".into(),
            StatusCode::UNAUTHORIZED,
            "invalid_client",
        ),
        (
            "client_secret",
            "malformed".into(),
            StatusCode::UNAUTHORIZED,
            "invalid_client",
        ),
        (
            "code",
            "malformed".into(),
            StatusCode::BAD_REQUEST,
            "invalid_grant",
        ),
        (
            "code",
            oauth_core::authorization_code::OAuthAuthorizationCode::new().to_string(),
            StatusCode::BAD_REQUEST,
            "invalid_grant",
        ),
        (
            "resource",
            "https://foreign.example".into(),
            StatusCode::BAD_REQUEST,
            "invalid_target",
        ),
        (
            "resource",
            "https://api.aura-historia.com".into(),
            StatusCode::BAD_REQUEST,
            "invalid_target",
        ),
    ] {
        let attempt = client.attempt("users:read").await;
        let code = client.code(&attempt, user).await;
        let mut form = client.exchange_form(&attempt, &code);
        form.insert(field.into(), value);
        assert_oauth_error(client.exchange(&form).await, status, expected).await;
    }
    let attempt = client.attempt("users:read").await;
    let code = client.code(&attempt, user).await;
    let wrong_client = other.exchange_form(&attempt, &code);
    assert_oauth_error(
        other.exchange(&wrong_client).await,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
    )
    .await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_reject_replayed_expired_and_concurrently_redeemed_codes() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    let attempt = client.attempt("users:read").await;
    let code = client.code(&attempt, user).await;
    let form = client.exchange_form(&attempt, &code);
    let (left, right) = tokio::join!(client.exchange(&form), client.exchange(&form));
    let mut statuses = [left.status().as_u16(), right.status().as_u16()];
    statuses.sort();
    assert_eq!([200, 400], statuses);
    assert_oauth_error(
        client.exchange(&form).await,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
    )
    .await;
    let code = client.code(&attempt, user).await;
    sqlx::query("UPDATE oauth_authorization_codes SET expires_at=now()-interval '1 second' WHERE authorization_code=$1")
        .bind(&code).execute(&get_postgres_client().await).await.unwrap();
    assert_oauth_error(
        client
            .exchange(&client.exchange_form(&attempt, &code))
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
    )
    .await;
}

async fn assert_oauth_error(response: reqwest::Response, status: StatusCode, code: &str) {
    assert_credentials_headers(&response);
    assert_eq!(status, response.status());
    assert_eq!(
        json!({"error":code}),
        response.json::<Value>().await.unwrap()
    );
}
