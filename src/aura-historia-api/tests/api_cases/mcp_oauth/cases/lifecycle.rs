use super::*;
use reqwest::Method;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_link_reauthorize_and_disconnect_at_the_same_public_url() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    let mcp = McpServer::start().await;
    assert_discovery(&mcp).await;
    assert_public_overview(&mcp).await;

    let missing = mcp.call("getUserAccount", json!({}), None).await;
    assert_eq!(StatusCode::UNAUTHORIZED, missing.status());
    assert!(
        missing.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("scope=\"users:read\"")
    );
    let read = client.link(user, "users:read").await;
    let read_token = read["access_token"].as_str().unwrap();
    let api_requests = api_support::api_request_count();
    let account = successful_result(
        mcp.call("getUserAccount", json!({}), Some(read_token))
            .await,
    )
    .await;
    assert_eq!(
        api_requests,
        api_support::api_request_count(),
        "MCP must call its in-process services without REST or token introspection requests"
    );
    assert_eq!(json!(user), account["userId"]);
    let rest = client
        .rest(Method::GET, "/v1/me/account", read_token, None)
        .await;
    assert_eq!(StatusCode::OK, rest.status());
    assert_eq!(account, rest.json::<Value>().await.unwrap());

    let denied = mcp
        .call(
            "updateUserAccount",
            json!({"firstName":"Ada"}),
            Some(read_token),
        )
        .await;
    assert_eq!(StatusCode::FORBIDDEN, denied.status());
    let challenge = denied.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("error=\"insufficient_scope\""));
    assert!(challenge.contains("scope=\"users:write\""));
    assert!(challenge.contains("resource_metadata=\"https://mcp.aura-historia.com/.well-known/oauth-protected-resource/v1\""));
    assert_eq!(
        StatusCode::FORBIDDEN,
        client
            .rest(
                Method::PATCH,
                "/v1/me/account",
                read_token,
                Some(&json!({"firstName":"Ada"}))
            )
            .await
            .status()
    );

    // Additional scope requires a new human approval; there is no refresh-token upgrade.
    let write = client.link(user, "users:read users:write").await;
    let write_token = write["access_token"].as_str().unwrap();
    let api_requests = api_support::api_request_count();
    let updated = successful_result(
        mcp.call(
            "updateUserAccount",
            json!({"firstName":"Ada","language":"de-DE"}),
            Some(write_token),
        )
        .await,
    )
    .await;
    assert_eq!("Ada", updated["firstName"]);
    assert_eq!(api_requests, api_support::api_request_count());
    assert_eq!("de", updated["language"]);
    assert_eq!(
        updated,
        client
            .rest(Method::GET, "/v1/me/account", write_token, None)
            .await
            .json::<Value>()
            .await
            .unwrap()
    );
    let rest_patch = client
        .rest(
            Method::PATCH,
            "/v1/me/account",
            write_token,
            Some(&json!({"lastName":"Lovelace"})),
        )
        .await;
    assert_eq!(StatusCode::OK, rest_patch.status());
    assert_eq!(
        rest_patch.json::<Value>().await.unwrap(),
        successful_result(
            mcp.call("getUserAccount", json!({}), Some(write_token))
                .await
        )
        .await
    );

    client.revoke(write_token).await;
    // Revocation is authoritative on the next check. Already-authorized work is not cancelled.
    for name in ["getUserAccount", "getPublicOverview"] {
        assert_eq!(
            StatusCode::UNAUTHORIZED,
            mcp.call(name, json!({}), Some(write_token)).await.status()
        );
    }
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        client
            .rest(Method::GET, "/v1/me/account", write_token, None)
            .await
            .status()
    );
    assert_public_overview(&mcp).await;
    assert_eq!(
        StatusCode::OK,
        mcp.call("getUserAccount", json!({}), Some(read_token))
            .await
            .status()
    );
    let reconnected = client.link(user, "users:read").await;
    assert_eq!(
        StatusCode::OK,
        mcp.call(
            "getUserAccount",
            json!({}),
            reconnected["access_token"].as_str()
        )
        .await
        .status()
    );
}

async fn assert_discovery(mcp: &McpServer) {
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/v1",
    ] {
        let response = mcp
            .http
            .get(format!("{}{path}", mcp.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()["cache-control"]);
        let body: Value = response.json().await.unwrap();
        assert_eq!(MCP_RESOURCE, body["resource"]);
        assert_eq!(
            json!(["https://api.aura-historia.com"]),
            body["authorization_servers"]
        );
    }
}

async fn assert_public_overview(mcp: &McpServer) {
    use product_listing_service::use_cases::RefreshPublicCatalogueOverviewUseCase;
    product_listing_service::use_cases::RefreshPublicCatalogueOverviewHandler::new(
        platform_postgres::SqlxUnitOfWork::new(mcp.pool.clone()),
        product_listing_postgres::SqlxPublicCatalogueOverviewRefresherFactory::new(),
    )
    .execute()
    .await
    .unwrap();
    let public = successful_result(mcp.call("getPublicOverview", json!({}), None).await).await;
    assert!(public["liveObjects"].is_number());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_handle_denial_and_untrusted_callback_without_issuing_a_credential() {
    let client = GenericClient::registered().await;
    let attempt = client.attempt("users:read").await;
    let state = &attempt.query["state"];
    // Browser denial is a client callback, not a backend approval request. Webapp rendering
    // is outside this fixture; no approval or exchange is made for a denied attempt.
    assert_eq!(
        Ok(None),
        attempt.callback(&format!("{CALLBACK}?error=access_denied&state={state}"))
    );
    for callback in [
        format!("{CALLBACK}?code=untrusted&state=wrong"),
        format!("https://foreign.example/callback?code=untrusted&state={state}"),
        format!("{CALLBACK}/suffix?code=untrusted&state={state}"),
        format!("{CALLBACK}?code=untrusted&state={state}&state={state}"),
        format!("{CALLBACK}?code=untrusted&error=access_denied&state={state}"),
        format!("{CALLBACK}?code=untrusted&state={state}#fragment"),
    ] {
        assert!(attempt.callback(&callback).is_err());
    }
    let pool = get_postgres_client().await;
    for sql in [
        "SELECT COUNT(*) FROM oauth_authorization_codes WHERE client_id=$1",
        "SELECT COUNT(*) FROM access_tokens WHERE oauth_client_id=$1",
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .bind(
                client
                    .id
                    .parse::<credential_core::oauth_client_id::OAuthClientId>()
                    .unwrap()
                    .as_uuid(),
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(0, count);
    }
}
