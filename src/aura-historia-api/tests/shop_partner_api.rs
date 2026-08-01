mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_shop, seed_user};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::shop_id::ShopId;
use std::collections::HashSet;
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_create_partner_application_for_existing_shop() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;
    let shop = seed_shop().await;

    let response = post_application(&token, shop.id()).await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    assert_eq!(
        serde_json::json!(user_id.to_string()),
        body["applicantUserId"]
    );
    assert_eq!(
        serde_json::json!(shop.id().to_string()),
        body["payload"]["shop_id"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_create_partner_application_for_new_shop() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;
    let suffix = PartnerShopApplicationId::new().to_string();
    let domain = format!("new-shop-{suffix}.example");

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "payload": {
                "type": "NEW",
                "shopName": format!("New Shop {suffix}"),
                "shopType": "COMMERCIAL_DEALER",
                "shopDomains": [domain],
                "shopUrl": format!("https://new-shop-{suffix}.example"),
                "shopImage": format!("https://new-shop-{suffix}.example/logo.svg")
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create new partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    assert_eq!(
        serde_json::json!(user_id.to_string()),
        body["applicantUserId"]
    );
    assert_eq!(serde_json::json!("new"), body["payload"]["type"]);
    assert!(
        body["payload"]["shop_id"].as_str().is_some(),
        "missing new shop id: {body}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_list_own_partner_applications() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;
    let application_id = create_application(&token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list partner applications API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body[0]["id"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_get_own_partner_application() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;
    let application_id = create_application(&token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body["id"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_delete_own_partner_application() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;
    let application_id = create_application(&token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/me/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete partner application API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_partner_application_read_when_id_is_invalid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_return_not_found_when_own_partner_application_is_missing() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications/{}",
            AURA_API.base_url(),
            PartnerShopApplicationId::new()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNER_SHOP_APPLICATION_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_list_partner_applications_when_actor_is_admin() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin list partner applications API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body[0]["id"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_get_partner_application_when_actor_is_admin() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin get partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body["id"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_mark_partner_application_in_review_when_actor_is_admin() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"taskToken": "task-token"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin patch partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body["id"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_decide_partner_application_when_actor_is_admin() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;
    let patched = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&serde_json::json!({"taskToken": "task-token"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin patch partner application API: {error}"));
    assert_eq!(reqwest::StatusCode::OK, patched.status());

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{}/decision",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"decision": "reject"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to decide partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body["id"]);
    assert_eq!(serde_json::json!("Rejected"), body["businessState"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_partner_applications_list_when_admin_access_token_lacks_scope() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin list partner applications API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_partner_applications_list_when_actor_is_not_admin() {
    let token = user_token().await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to admin list partner applications API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_admin_partner_application_patch_when_task_token_is_missing() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to admin patch invalid partner application API: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_require_auth_for_partner_applications() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to list missing auth partner application API: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

async fn create_application(
    token: &user_core::access_token::RawAccessToken,
    shop_id: ShopId,
) -> String {
    let response = post_application(token, shop_id).await;
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing partner application id"))
        .to_owned()
}

async fn post_application(
    token: &user_core::access_token::RawAccessToken,
    shop_id: ShopId,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "payload": {"type": "existing", "shop_id": shop_id.to_string()}
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create partner application API: {error}"))
}

async fn admin_token() -> user_core::access_token::RawAccessToken {
    let user_id = seed_user("ADMIN").await;
    seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await
}

async fn user_token() -> user_core::access_token::RawAccessToken {
    let user_id = seed_user("USER").await;
    seed_access_token_for(
        user_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite]),
    )
    .await
}
