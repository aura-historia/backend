mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_shop, seed_user};
use common::partner_shop_application_id::PartnerShopApplicationId;
use shop_core::shop_id::ShopId;
use std::collections::HashSet;
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
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
        serde_json::json!(shop.id().to_string()),
        body["payload"]["shopId"]
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
    assert_eq!(serde_json::json!("new"), body["payload"]["type"]);
    assert!(
        body["payload"]["shopId"].as_str().is_some(),
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
        .json(&serde_json::json!({"decision": "REJECT"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to decide partner application API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(application_id), body["id"]);
    assert_eq!(serde_json::json!("REJECTED"), body["businessState"]);
    assert!(body.get("executionState").is_none());
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
async fn should_mark_partner_application_in_review() {
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
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to mark application in review: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("IN_REVIEW"), body["businessState"]);
    assert!(body.get("executionState").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_lowercase_partner_application_decision() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;
    let reviewed = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{}",
            AURA_API.base_url(),
            application_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to mark application in review: {error}"));
    assert_eq!(reqwest::StatusCode::OK, reviewed.status());

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{application_id}/decision",
            AURA_API.base_url(),
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"decision": "reject"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to submit lowercase decision: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_decision_before_application_is_in_review() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{application_id}/decision",
            AURA_API.base_url(),
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"decision": "APPROVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to decide submitted application: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_opposite_decision_after_terminal_application_state() {
    let admin_token = admin_token().await;
    let user_token = user_token().await;
    let application_id = create_application(&user_token, seed_shop().await.id()).await;

    let reviewed = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{application_id}",
            AURA_API.base_url(),
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to mark application in review: {error}"));
    assert_eq!(reqwest::StatusCode::OK, reviewed.status());

    let rejected = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{application_id}/decision",
            AURA_API.base_url(),
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&serde_json::json!({"decision": "REJECT"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject application: {error}"));
    assert_eq!(reqwest::StatusCode::OK, rejected.status());

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{application_id}/decision",
            AURA_API.base_url(),
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"decision": "APPROVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to submit opposite decision: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_approve_existing_application_and_grant_partner_shop_membership() {
    let admin_token = admin_token().await;
    let applicant_id = seed_user("USER").await;
    let applicant_token = seed_access_token_for(
        applicant_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite, Scope::PartnerShopsRead]),
    )
    .await;
    let shop = seed_shop().await;
    let application_id = create_application(&applicant_token, shop.id()).await;

    mark_in_review(&admin_token, &application_id).await;
    let (status, body) =
        json_response(decide(&admin_token, &application_id, "APPROVE").await).await;

    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("APPROVED"), body["businessState"]);

    let (status, body) = json_response(partner_shops(&applicant_token).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!(shop.id().to_string()), body[0]["shopId"]);
    assert_eq!(serde_json::json!("PARTNERED"), body[0]["partnerStatus"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_approve_new_application_publish_shop_and_grant_partner_shop_membership() {
    let admin_token = admin_token().await;
    let applicant_id = seed_user("USER").await;
    let applicant_token = seed_access_token_for(
        applicant_id,
        HashSet::from([Scope::PartnerShopApplicationsWrite, Scope::PartnerShopsRead]),
    )
    .await;
    let (application_id, shop_id) = create_new_application(&applicant_token).await;

    mark_in_review(&admin_token, &application_id).await;
    let (status, body) =
        json_response(decide(&admin_token, &application_id, "APPROVE").await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("APPROVED"), body["businessState"]);

    let (status, body) = json_response(get_shop(&shop_id).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!(shop_id), body["shopId"]);
    assert_eq!(serde_json::json!("PARTNERED"), body["partnerStatus"]);

    let (status, body) = json_response(partner_shops(&applicant_token).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!(shop_id), body[0]["shopId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_new_application_and_keep_draft_shop_non_public() {
    let admin_token = admin_token().await;
    let applicant_token = user_token().await;
    let (application_id, shop_id) = create_new_application(&applicant_token).await;

    mark_in_review(&admin_token, &application_id).await;
    let (status, body) = json_response(decide(&admin_token, &application_id, "REJECT").await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("REJECTED"), body["businessState"]);

    let (status, body) = json_response(get_shop(&shop_id).await).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "SHOP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_existing_application_without_hiding_shop() {
    let admin_token = admin_token().await;
    let applicant_token = user_token().await;
    let shop = seed_shop().await;
    let application_id = create_application(&applicant_token, shop.id()).await;

    mark_in_review(&admin_token, &application_id).await;
    let (status, body) = json_response(decide(&admin_token, &application_id, "REJECT").await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("REJECTED"), body["businessState"]);

    let (status, body) = json_response(get_shop(&shop.id().to_string()).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!(shop.id().to_string()), body["shopId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_withdraw_new_application_and_keep_draft_shop_non_public() {
    let applicant_token = user_token().await;
    let (application_id, shop_id) = create_new_application(&applicant_token).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/me/partner-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(applicant_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to withdraw new partner application: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let (status, body) =
        json_response(own_application(&applicant_token, &application_id).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("WITHDRAWN"), body["businessState"]);

    let (status, body) = json_response(get_shop(&shop_id).await).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "SHOP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_withdraw_existing_application_without_hiding_shop() {
    let applicant_token = user_token().await;
    let shop = seed_shop().await;
    let application_id = create_application(&applicant_token, shop.id()).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/me/partner-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(applicant_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to withdraw existing partner application: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let (status, body) = json_response(get_shop(&shop.id().to_string()).await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!(shop.id().to_string()), body["shopId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_replay_matching_terminal_decision_without_changing_result() {
    let admin_token = admin_token().await;
    let applicant_token = user_token().await;
    let application_id = create_application(&applicant_token, seed_shop().await.id()).await;

    mark_in_review(&admin_token, &application_id).await;
    let first = decide(&admin_token, &application_id, "REJECT").await;
    assert_eq!(reqwest::StatusCode::OK, first.status());
    let (status, body) = json_response(decide(&admin_token, &application_id, "REJECT").await).await;
    assert_eq!(reqwest::StatusCode::OK, status, "{body}");
    assert_eq!(serde_json::json!("REJECTED"), body["businessState"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_allow_only_one_of_concurrent_opposite_decisions() {
    let admin_token = admin_token().await;
    let applicant_token = user_token().await;
    let application_id = create_application(&applicant_token, seed_shop().await.id()).await;
    mark_in_review(&admin_token, &application_id).await;

    let (approve, reject) = tokio::join!(
        decide(&admin_token, &application_id, "APPROVE"),
        decide(&admin_token, &application_id, "REJECT"),
    );
    let statuses = [approve.status(), reject.status()];
    assert!(statuses.contains(&reqwest::StatusCode::OK), "{statuses:?}");
    assert!(
        statuses.contains(&reqwest::StatusCode::CONFLICT),
        "{statuses:?}"
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

async fn create_new_application(
    token: &user_core::access_token::RawAccessToken,
) -> (String, String) {
    let suffix = PartnerShopApplicationId::new().to_string();
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/partner-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "payload": {
                "type": "NEW",
                "shopName": format!("New Shop {suffix}"),
                "shopType": "COMMERCIAL_DEALER",
                "shopDomains": [format!("new-shop-{suffix}.example")]
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create new partner application: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    let application_id = body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing partner application id: {body}"))
        .to_owned();
    let shop_id = body["payload"]["shopId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing new shop id: {body}"))
        .to_owned();
    (application_id, shop_id)
}

async fn mark_in_review(token: &user_core::access_token::RawAccessToken, application_id: &str) {
    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partner-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to mark partner application in review: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());
}

async fn decide(
    token: &user_core::access_token::RawAccessToken,
    application_id: &str,
    decision: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/partner-applications/{application_id}/decision",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"decision": decision}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to decide partner application: {error}"))
}

async fn get_shop(shop_id: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}/api/v1/shops/{shop_id}", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get shop: {error}"))
}

async fn partner_shops(token: &user_core::access_token::RawAccessToken) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}/api/v1/me/partner-shops", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list partner shops: {error}"))
}

async fn own_application(
    token: &user_core::access_token::RawAccessToken,
    application_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/partner-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get own partner application: {error}"))
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
