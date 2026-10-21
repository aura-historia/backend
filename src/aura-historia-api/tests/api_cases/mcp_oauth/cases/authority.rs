use super::*;
use reqwest::Method;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_keep_concurrent_users_ownership_and_roles_identical_to_rest() {
    let client = GenericClient::registered().await;
    let alice = api_support::seed_user("USER").await;
    let bob = api_support::seed_user("USER").await;
    let alice_grant = client.link(alice, "users:read users:write").await;
    let bob_grant = client.link(bob, "users:read users:write").await;
    let alice_token = alice_grant["access_token"].as_str().unwrap();
    let bob_token = bob_grant["access_token"].as_str().unwrap();
    let mcp = McpServer::start().await;
    let (left, right) = tokio::join!(
        mcp.call(
            "updateUserAccount",
            json!({"firstName":"Alice"}),
            Some(alice_token)
        ),
        mcp.call(
            "updateUserAccount",
            json!({"firstName":"Bob"}),
            Some(bob_token)
        ),
    );
    let left = successful_result(left).await;
    let right = successful_result(right).await;
    assert_eq!(json!(alice), left["userId"]);
    assert_eq!(json!(bob), right["userId"]);
    for _ in 0..4 {
        let (left, right) = tokio::join!(
            mcp.call("getUserAccount", json!({}), Some(alice_token)),
            mcp.call("getUserAccount", json!({}), Some(bob_token)),
        );
        assert_eq!("Alice", successful_result(left).await["firstName"]);
        assert_eq!("Bob", successful_result(right).await["firstName"]);
    }
    for (token, expected) in [(alice_token, alice), (bob_token, bob)] {
        let rest = client
            .rest(Method::GET, "/v1/me/account", token, None)
            .await;
        assert_eq!(
            json!(expected),
            rest.json::<Value>().await.unwrap()["userId"]
        );
        // users:read does not grant an administrator role.
        assert_eq!(
            StatusCode::FORBIDDEN,
            client
                .rest(Method::GET, &format!("/v1/admin/users/{bob}"), token, None)
                .await
                .status()
        );
    }
    assert_address_ownership(&client, &mcp, alice_token, bob_token).await;
    client.revoke(alice_token).await;
    assert_eq!(
        StatusCode::OK,
        mcp.call("getUserAccount", json!({}), Some(bob_token))
            .await
            .status()
    );
}

async fn assert_address_ownership(
    client: &GenericClient,
    mcp: &McpServer,
    alice_token: &str,
    bob_token: &str,
) {
    // Schema-once test cleanup truncates provider controls after every scenario. Seed an
    // explicit local profile, including when this scenario is the first after migration.
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&mcp.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('GENERIC_MCP_TEST',true)")
        .execute(&mcp.pool).await.unwrap();
    let created = successful_result(
        mcp.call(
            "createUserAddress",
            json!({
                "label":"Alice home", "geography":{
                    "referenceRelease":"iso-codes-4.20.1", "addressText":"Synthetic private home",
                    "country":"DE", "subdivision":"DE-BE", "postalCode":"10115"
                }
            }),
            Some(alice_token),
        )
        .await,
    )
    .await;
    let id = created["addressId"].as_str().unwrap();
    let owned = client
        .rest(
            Method::GET,
            &format!("/v1/me/addresses/{id}"),
            alice_token,
            None,
        )
        .await;
    assert_eq!(StatusCode::OK, owned.status());
    assert_eq!(created, owned.json::<Value>().await.unwrap());
    let foreign = mcp
        .call("getUserAddress", json!({"addressId":id}), Some(bob_token))
        .await;
    assert_eq!(StatusCode::OK, foreign.status());
    let foreign: Value = foreign.json().await.unwrap();
    assert_eq!(true, foreign["result"]["isError"]);
    assert_eq!("NOT_FOUND", foreign["result"]["content"][0]["text"]);
    assert_eq!(
        StatusCode::NOT_FOUND,
        client
            .rest(
                Method::GET,
                &format!("/v1/me/addresses/{id}"),
                bob_token,
                None
            )
            .await
            .status()
    );
    let bob_addresses = successful_result(
        mcp.call("listUserAddresses", json!({}), Some(bob_token))
            .await,
    )
    .await;
    assert_eq!(json!([]), bob_addresses["items"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_reject_expiry_suspension_and_auth_store_outage_on_the_next_authorization_check() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    let grant = client.link(user, "users:read").await;
    let token = grant["access_token"].as_str().unwrap();
    let mcp = McpServer::start().await;
    assert_shared_status(&client, &mcp, token, StatusCode::OK).await;
    sqlx::query("UPDATE access_tokens SET expires_at=now()+interval '1 hour' WHERE user_id=$1")
        .bind(user.as_uuid())
        .execute(&mcp.pool)
        .await
        .unwrap();
    assert_shared_status(&client, &mcp, token, StatusCode::OK).await;
    sqlx::query("UPDATE access_tokens SET expires_at=now()-interval '1 second' WHERE user_id=$1")
        .bind(user.as_uuid())
        .execute(&mcp.pool)
        .await
        .unwrap();
    assert_shared_status(&client, &mcp, token, StatusCode::UNAUTHORIZED).await;
    sqlx::query("UPDATE access_tokens SET expires_at=NULL WHERE user_id=$1")
        .bind(user.as_uuid())
        .execute(&mcp.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(user.as_uuid())
        .execute(&mcp.pool)
        .await
        .unwrap();
    assert_shared_status(&client, &mcp, token, StatusCode::UNAUTHORIZED).await;
    sqlx::query("UPDATE users SET suspended=false WHERE user_id=$1")
        .bind(user.as_uuid())
        .execute(&mcp.pool)
        .await
        .unwrap();
    assert_shared_status(&client, &mcp, token, StatusCode::OK).await;
    // Close only this MCP fixture's dedicated pool, leaving the API/other tests untouched.
    mcp.pool.close().await;
    let outage = mcp.call("getUserAccount", json!({}), Some(token)).await;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, outage.status());
    assert_eq!("1", outage.headers()["retry-after"]);
    assert!(outage.headers().get("www-authenticate").is_none());
    assert_eq!(
        StatusCode::OK,
        client
            .rest(Method::GET, "/v1/me/account", token, None)
            .await
            .status()
    );
}

async fn assert_shared_status(
    client: &GenericClient,
    mcp: &McpServer,
    token: &str,
    expected: StatusCode,
) {
    assert_eq!(
        expected,
        mcp.call("getUserAccount", json!({}), Some(token))
            .await
            .status()
    );
    assert_eq!(
        expected,
        client
            .rest(Method::GET, "/v1/me/account", token, None)
            .await
            .status()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_revoke_the_next_check_without_retroactively_cancelling_an_authorized_write() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("USER").await;
    let grant = client.link(user, "users:read users:write").await;
    let token = grant["access_token"].as_str().unwrap();
    let mcp = McpServer::start().await;
    let mut lock = mcp.pool.begin().await.unwrap();
    let lock_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    sqlx::query("SELECT user_id FROM users WHERE user_id=$1 FOR UPDATE")
        .bind(user.as_uuid())
        .execute(&mut *lock)
        .await
        .unwrap();
    let write = mcp.call(
        "updateUserAccount",
        json!({"firstName":"Already authorized"}),
        Some(token),
    );
    tokio::pin!(write);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut write => panic!("write unexpectedly completed while its user row was locked"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%UPDATE users SET%')")
                .bind(lock_pid).fetch_one(&mcp.pool).await.unwrap();
            if blocked { break; }
        }
    }).await.expect("the real profile mutation must reach its locked write after authorization");
    client.revoke(token).await;
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        mcp.call("getUserAccount", json!({}), Some(token))
            .await
            .status()
    );
    lock.commit().await.unwrap();
    let completed = successful_result(write.await).await;
    assert_eq!("Already authorized", completed["firstName"]);
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        mcp.call("getUserAccount", json!({}), Some(token))
            .await
            .status()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
#[ignore = "executed by the isolated generic OAuth/MCP acceptance runner"]
async fn should_preserve_first_party_no_expiry_and_reject_delegated_consent() {
    let client = GenericClient::registered().await;
    let user = api_support::seed_user("ADMIN").await;
    let delegated = client
        .link(user, "users:read users:write access-tokens:write")
        .await;
    let attempt = client.attempt("users:read").await;
    let response = client
        .approve(&attempt, delegated["access_token"].as_str().unwrap())
        .await;
    assert_eq!(StatusCode::FORBIDDEN, response.status());
    assert!(response.headers().get("location").is_none());
    client::assert_credentials_headers(&response);
    let id = client
        .id
        .parse::<credential_core::oauth_client_id::OAuthClientId>()
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM oauth_authorization_codes WHERE client_id=$1")
            .bind(id.as_uuid())
            .fetch_one(&get_postgres_client().await)
            .await
            .unwrap();
    assert_eq!(0, count);
    // First-party human consent still deliberately produces a nonexpiring grant.
    let human = client.link(user, "users:read").await;
    assert!(human.get("expires_in").is_none());
    assert!(human.get("refresh_token").is_none());
}
