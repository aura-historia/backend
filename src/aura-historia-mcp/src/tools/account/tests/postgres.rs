use super::*;
use crate::{AuraTokenGate, TransportConfig};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::access_token::{RawAccessToken, Scope};
use user_service::use_cases::{
    AuthenticateAccessTokenError, AuthenticateAccessTokenRequest, AuthenticateAccessTokenResult,
    AuthenticateAccessTokenUseCase,
};

const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
async fn seed(pool: &sqlx::PgPool) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(id.into_uuid())
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    id
}
fn both(owner: UserId) -> InvocationContext {
    context(
        owner,
        [
            CredentialCapability::UsersRead,
            CredentialCapability::UsersWrite,
        ],
    )
}
async fn assert_error(
    registry: &Registry,
    name: &str,
    input: Value,
    ctx: &InvocationContext,
    expected: fn(&OperationError) -> bool,
) {
    let result = registry.get(name).unwrap().call(input, ctx.clone()).await;
    assert!(result.as_ref().is_err_and(expected), "{name}: {result:?}");
}

#[aura_integration_test(services = [SCHEMA])]
async fn should_apply_profile_patches_only_to_the_delegated_owner_and_preserve_scope_checks() {
    let pool = get_postgres_client().await;
    let owner = seed(&pool).await;
    let foreign = seed(&pool).await;
    let runtime = Runtime::from_pool(pool);
    let registry = registry(&runtime);
    let ctx = both(owner);
    let initial = invoke(&registry, "getUserAccount", json!({}), &ctx).await;
    assert_eq!(json!(owner), initial["userId"]);
    assert!(initial.get("firstName").is_none());
    let read = context(owner, [CredentialCapability::UsersRead]);
    let update = json!({"firstName":"Ada","lastName":"Lovelace","language":"de-CH","currency":"ARS","measurementUnit":"IMPERIAL","showUnassessedOrSensitiveContent":true,"userId":foreign,"role":"ADMIN","marketingEmailConsent":true});
    assert_error(&registry, "updateUserAccount", update.clone(), &read, |e| {
        matches!(e, OperationError::Forbidden)
    })
    .await;
    let result = invoke(&registry, "updateUserAccount", update, &ctx).await;
    assert_eq!(json!(owner), result["userId"]);
    assert_eq!(json!("Ada"), result["firstName"]);
    assert_eq!(json!("Lovelace"), result["lastName"]);
    assert_eq!(json!("de"), result["language"]);
    assert_eq!(json!("ARS"), result["currency"]);
    assert_eq!(json!("IMPERIAL"), result["measurementUnit"]);
    assert_eq!(json!(true), result["showUnassessedOrSensitiveContent"]);
    assert_eq!(json!(false), result["marketingEmailConsent"]);
    assert_eq!(json!("USER"), result["role"]);
    let foreign_result = invoke(&registry, "getUserAccount", json!({}), &both(foreign)).await;
    assert!(foreign_result.get("firstName").is_none());
    assert_eq!(
        result,
        invoke(&registry, "updateUserAccount", json!({}), &ctx).await
    );
    let cleared=invoke(&registry,"updateUserAccount",json!({"firstName":null,"lastName":null,"language":null,"currency":null,"measurementUnit":null}),&ctx).await;
    for field in [
        "firstName",
        "lastName",
        "language",
        "currency",
        "measurementUnit",
    ] {
        assert!(cleared.get(field).is_none());
    }
    assert_eq!(json!(true), cleared["showUnassessedOrSensitiveContent"]);
    assert_error(
        &registry,
        "updateUserAccount",
        json!({"showUnassessedOrSensitiveContent":null}),
        &ctx,
        |e| matches!(e, OperationError::InvalidArguments(_)),
    )
    .await;
    assert_eq!(
        json!(false),
        invoke(
            &registry,
            "updateUserAccount",
            json!({"showUnassessedOrSensitiveContent":false}),
            &ctx
        )
        .await["showUnassessedOrSensitiveContent"]
    );
    assert_error(
        &registry,
        "getUserAccount",
        json!({}),
        &context(owner, [CredentialCapability::UsersWrite]),
        |e| matches!(e, OperationError::Forbidden),
    )
    .await;
}

#[aura_integration_test(services = [SCHEMA])]
async fn should_preserve_owned_address_pagination_cas_destinations_and_http_scope_challenges() {
    let pool = get_postgres_client().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('MCP_ACCOUNT_TEST',true)").execute(&pool).await.unwrap();
    let owner = seed(&pool).await;
    let foreign = seed(&pool).await;
    let runtime = Runtime::from_pool(pool);
    let registry = registry(&runtime);
    let ctx = both(owner);
    let first = invoke(&registry, "createUserAddress", input(), &ctx).await;
    let duplicate = invoke(&registry, "createUserAddress", input(), &ctx).await;
    let foreign_address = invoke(&registry, "createUserAddress", input(), &both(foreign)).await;
    assert_ne!(first["addressId"], duplicate["addressId"]);
    let id: UserAddressId = serde_json::from_value(first["addressId"].clone()).unwrap();
    let read = context(owner, [CredentialCapability::UsersRead]);
    let address = invoke(&registry, "getUserAddress", json!({"addressId":id}), &read).await;
    assert_eq!(input()["geography"], address["geography"]);
    assert_eq!(json!(1), address["version"]);
    assert_eq!(
        json!({"status":"PENDING","derived":null,"attribution":null}),
        address["resolution"]
    );
    let mut seen = BTreeSet::new();
    let mut after = Value::Null;
    loop {
        let page = invoke(
            &registry,
            "listUserAddresses",
            json!({"limit":1,"after":after}),
            &read,
        )
        .await;
        for item in page["items"].as_array().unwrap() {
            assert!(seen.insert(item["addressId"].as_str().unwrap().to_owned()));
        }
        after = page["next"].clone();
        if after.is_null() {
            break;
        }
    }
    assert_eq!(
        BTreeSet::from([
            first["addressId"].as_str().unwrap().to_owned(),
            duplicate["addressId"].as_str().unwrap().to_owned()
        ]),
        seen
    );
    assert_error(
        &registry,
        "getUserAddress",
        json!({"addressId":id}),
        &both(foreign),
        |e| matches!(e, OperationError::NotFound),
    )
    .await;
    for (name, input) in requests(id)
        .into_iter()
        .filter(|(name, _)| matches!(*name, "updateUserAddress" | "deleteUserAddress"))
    {
        assert_error(&registry, name, input, &both(foreign), |e| {
            matches!(e, OperationError::NotFound)
        })
        .await;
    }
    assert_eq!(
        address,
        invoke(&registry, "getUserAddress", json!({"addressId":id}), &read).await
    );
    let write_only = context(owner, [CredentialCapability::UsersWrite]);
    for (name, input) in requests(id)
        .into_iter()
        .filter(|(name, _)| registry.get(name).unwrap().policy().read_only)
    {
        assert_error(&registry, name, input, &write_only, |e| {
            matches!(e, OperationError::Forbidden)
        })
        .await;
    }
    for (name, input) in requests(id)
        .into_iter()
        .filter(|(name, _)| !registry.get(name).unwrap().policy().read_only)
    {
        assert_error(&registry, name, input, &read, |e| {
            matches!(e, OperationError::Forbidden)
        })
        .await;
    }
    let mut replacement = input();
    replacement["label"] = json!("Replacement");
    replacement["geography"]["addressText"] = json!("Replacement private home");
    replacement["addressId"] = json!(id);
    replacement["expectedVersion"] = json!(1);
    let updated = invoke(&registry, "updateUserAddress", replacement.clone(), &ctx).await;
    assert_eq!(json!(2), updated["version"]);
    assert_eq!(json!(2), updated["inputRevision"]);
    assert_error(&registry, "updateUserAddress", replacement, &ctx, |e| {
        matches!(e, OperationError::Conflict)
    })
    .await;
    assert_error(
        &registry,
        "deleteUserAddress",
        json!({"addressId":id,"expectedVersion":1}),
        &ctx,
        |e| matches!(e, OperationError::Conflict),
    )
    .await;
    exercise_destinations(&registry, &ctx, id, foreign_address["addressId"].clone()).await;
    exercise_invalid_inputs(&registry, &ctx, id).await;
    exercise_http_scopes(&runtime, owner, id).await;
    let selected = invoke(
        &registry,
        "setBrowsingDestination",
        json!({"expectedVersion":5,"destination":{"kind":"ADDRESS","addressId":id}}),
        &ctx,
    )
    .await;
    assert_eq!(json!(6), selected["version"]);
    assert_eq!(
        json!(true),
        invoke(&registry, "getUserAddress", json!({"addressId":id}), &ctx).await["preferred"]
    );
    assert_eq!(
        json!({"result":null}),
        invoke(
            &registry,
            "deleteUserAddress",
            json!({"addressId":id,"expectedVersion":2}),
            &ctx
        )
        .await
    );
    assert_eq!(
        json!({"version":7,"destination":{"kind":"NONE"}}),
        invoke(&registry, "getBrowsingDestination", json!({}), &ctx).await
    );
    assert_error(
        &registry,
        "getUserAddress",
        json!({"addressId":id}),
        &ctx,
        |e| matches!(e, OperationError::NotFound),
    )
    .await;
    assert_eq!(
        1,
        invoke(&registry, "listUserAddresses", json!({}), &ctx).await["items"]
            .as_array()
            .unwrap()
            .len()
    );
    exercise_page_bounds(&registry, &ctx).await;
}

async fn exercise_page_bounds(registry: &Registry, ctx: &InvocationContext) {
    for _ in 0..25 {
        invoke(registry, "createUserAddress", input(), ctx).await;
    }
    let default_page = invoke(registry, "listUserAddresses", json!({}), ctx).await;
    assert_eq!(25, default_page["items"].as_array().unwrap().len());
    assert!(default_page["next"].is_string());
    assert_eq!(
        default_page,
        invoke(registry, "listUserAddresses", json!({"limit":null}), ctx).await
    );
    let last = invoke(
        registry,
        "listUserAddresses",
        json!({"after":default_page["next"]}),
        ctx,
    )
    .await;
    assert_eq!(1, last["items"].as_array().unwrap().len());
    assert!(last["next"].is_null());
    let complete = invoke(registry, "listUserAddresses", json!({"limit":100}), ctx).await;
    assert_eq!(26, complete["items"].as_array().unwrap().len());
    assert!(complete["next"].is_null());
    for limit in [101, u32::MAX] {
        assert_error(
            registry,
            "listUserAddresses",
            json!({"limit":limit}),
            ctx,
            |e| matches!(e, OperationError::InvalidInput),
        )
        .await;
    }
}

async fn exercise_destinations(
    registry: &Registry,
    ctx: &InvocationContext,
    id: UserAddressId,
    foreign: Value,
) {
    assert_eq!(
        json!({"version":1,"destination":{"kind":"NONE"}}),
        invoke(registry, "getBrowsingDestination", json!({}), ctx).await
    );
    assert_error(
        registry,
        "setBrowsingDestination",
        json!({"expectedVersion":1,"destination":{"kind":"ADDRESS","addressId":foreign}}),
        ctx,
        |e| matches!(e, OperationError::NotFound),
    )
    .await;
    for (version, destination) in [
        (1, json!({"kind":"ADDRESS","addressId":id})),
        (
            2,
            json!({"kind":"COUNTRY_POSTAL","country":"DE","postalCode":"10115"}),
        ),
        (3, json!({"kind":"COUNTRY_POSTAL","country":"FR"})),
        (4, json!({"kind":"NONE"})),
    ] {
        let result = invoke(
            registry,
            "setBrowsingDestination",
            json!({"expectedVersion":version,"destination":destination}),
            ctx,
        )
        .await;
        assert_eq!(json!(version + 1), result["version"]);
        assert_eq!(destination["kind"], result["destination"]["kind"]);
        if version == 2 {
            assert_eq!(destination, result["destination"]);
        }
    }
    assert_error(
        registry,
        "setBrowsingDestination",
        json!({"expectedVersion":1,"destination":{"kind":"NONE"}}),
        ctx,
        |e| matches!(e, OperationError::Conflict),
    )
    .await;
}
async fn exercise_invalid_inputs(registry: &Registry, ctx: &InvocationContext, id: UserAddressId) {
    let mut country_only = input();
    country_only["geography"]
        .as_object_mut()
        .unwrap()
        .remove("addressText");
    assert_error(registry, "createUserAddress", country_only, ctx, |e| {
        matches!(e, OperationError::InvalidInput)
    })
    .await;
    let mut changed_owner = input();
    changed_owner["owner"] = json!(UserId::new());
    assert_error(registry, "createUserAddress", changed_owner, ctx, |e| {
        matches!(e, OperationError::InvalidArguments(_))
    })
    .await;
    for (name, input) in [
        ("getUserAddress", json!({"addressId":id.as_uuid()})),
        (
            "updateUserAddress",
            json!({"addressId":id,"expectedVersion":2,"label":"Home","geography":input()["geography"],"owner":UserId::new()}),
        ),
        (
            "deleteUserAddress",
            json!({"addressId":id,"expectedVersion":2,"userId":UserId::new()}),
        ),
        (
            "setBrowsingDestination",
            json!({"expectedVersion":5,"destination":{"kind":"NONE","country":"DE"}}),
        ),
        (
            "setBrowsingDestination",
            json!({"expectedVersion":5,"destination":{"kind":"COUNTRY_POSTAL","country":"XX"}}),
        ),
    ] {
        assert_error(registry, name, input, ctx, |e| {
            matches!(e, OperationError::InvalidArguments(_))
        })
        .await;
    }
    assert_error(
        registry,
        "listUserAddresses",
        json!({"limit":0}),
        ctx,
        |e| matches!(e, OperationError::InvalidInput),
    )
    .await;
    for version in [0, i64::MAX as u64 + 1, u64::MAX] {
        for (name, input) in [
            (
                "updateUserAddress",
                json!({"addressId":id,"expectedVersion":version,"label":"Home","geography":input()["geography"]}),
            ),
            (
                "deleteUserAddress",
                json!({"addressId":id,"expectedVersion":version}),
            ),
            (
                "setBrowsingDestination",
                json!({"expectedVersion":version,"destination":{"kind":"NONE"}}),
            ),
        ] {
            let schema =
                serde_json::to_value(registry.get(name).unwrap().tool().input_schema.as_ref())
                    .unwrap();
            assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&input));
            assert_error(registry, name, input, ctx, |e| {
                matches!(e, OperationError::InvalidInput)
            })
            .await;
        }
    }
}

struct ReadToken(UserId);
#[async_trait::async_trait]
impl AuthenticateAccessTokenUseCase for ReadToken {
    async fn execute(
        &self,
        ctx: &OperationContext,
        _: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        assert_eq!(Principal::Anonymous, ctx.principal);
        Ok(AuthenticateAccessTokenResult {
            user_id: self.0,
            scopes: [Scope::UsersRead].into(),
        })
    }
}
async fn exercise_http_scopes(runtime: &Runtime, owner: UserId, id: UserAddressId) {
    let config = TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    };
    let app = crate::router(
        config.clone(),
        registry(runtime),
        Arc::new(AuraTokenGate::new(
            Arc::new(ReadToken(owner)),
            config.resource_policy,
        )),
    )
    .unwrap();
    let token = String::from(RawAccessToken::new());
    for (name, input) in requests(id) {
        let mut params = json!({"name":name,"arguments":input});
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"account-fixture","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params});
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/v1")
                .header("host", "mcp.example.test")
                .header("content-type", "application/json")
                .header("accept", "application/json,text/event-stream")
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call")
                .header("mcp-name", name)
        };
        let missing = app
            .clone()
            .oneshot(request().body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(StatusCode::UNAUTHORIZED, missing.status());
        let response = app
            .clone()
            .oneshot(
                request()
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!("no-store", response.headers()["cache-control"]);
        let read = registry(runtime).get(name).unwrap().policy().read_only;
        if read {
            assert_eq!(StatusCode::OK, response.status());
            let result: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(result.get("error").is_none(), "{result}");
            assert_ne!(json!(true), result["result"]["isError"], "{result}");
        } else {
            assert_eq!(StatusCode::FORBIDDEN, response.status());
            assert!(
                response.headers()["www-authenticate"]
                    .to_str()
                    .unwrap()
                    .contains("scope=\"users:write\"")
            );
        }
    }
}
