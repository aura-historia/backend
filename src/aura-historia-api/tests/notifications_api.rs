mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_user};
use common::user_id::UserId;
use serde_json::Value;
use std::collections::HashSet;
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
    get_postgres_client,
};
use user_core::access_token::RawAccessToken;
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_require_valid_authentication_for_notifications() {
    let client = reqwest::Client::new();

    let missing = client
        .get(notifications_path())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications without auth: {error}"));
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let invalid = client
        .get(notifications_path())
        .bearer_auth("not-an-aura-access-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications with invalid auth: {error}"));
    let (invalid_status, invalid_body) = json_response(invalid).await;
    assert_problem(
        invalid_status,
        &invalid_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_list_canonical_notifications_without_legacy_fields_and_follow_cursor() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, false).await;
    let client = reqwest::Client::new();

    let response = client
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "1")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(serde_json::json!(1), body["size"]);
    assert!(body.get("total").is_none());

    let item = &body["items"][0];
    let first_page_id = match item["notificationId"].as_str() {
        Some(value) => value,
        None => panic!("notification list item is missing notificationId"),
    };
    assert!(
        [
            first_notification_id.to_string(),
            second_notification_id.to_string()
        ]
        .contains(&first_page_id.to_owned())
    );
    assert_eq!(serde_json::json!(false), item["seen"]);
    assert_eq!(
        serde_json::json!("PARTNER_APPLICATION_APPROVED"),
        item["kind"]
    );
    assert!(
        item["payload"]["partnerShopApplicationId"]
            .as_str()
            .is_some(),
        "payload is missing partnerShopApplicationId"
    );
    assert_eq!(serde_json::json!("APPROVED"), item["payload"]["decision"]);
    for legacy_field in [
        "originEventId",
        "external",
        "createdBy",
        "updatedBy",
        "userId",
        "notificationType",
    ] {
        assert!(item.get(legacy_field).is_none(), "found {legacy_field}");
    }
    for legacy_payload_field in ["type", "shopName", "image", "partnerApplicationPayload"] {
        assert!(
            item["payload"].get(legacy_payload_field).is_none(),
            "found payload.{legacy_payload_field}"
        );
    }

    let cursor = match body.get("searchAfter") {
        Some(value) => value.to_string(),
        None => panic!("notification list page is missing searchAfter cursor"),
    };
    assert_eq!(serde_json::json!(first_page_id), body["searchAfter"][1]);
    assert!(body["searchAfter"][0].as_str().is_some());

    let response = client
        .get(notifications_path())
        .bearer_auth(String::from(token))
        .query(&[("size", "1"), ("searchAfter", cursor.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to follow notification cursor: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_ne!(
        serde_json::json!(first_page_id),
        body["items"][0]["notificationId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_malformed_notification_id() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!("{}/not-a-uuid", notifications_path()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch malformed notification ID: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_empty_notification_batch() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;

    let response = reqwest::Client::new()
        .patch(notifications_path())
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notificationIds": [], "seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch empty notification batch: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_update_one_notification_seen_state() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .patch(notification_path(notification_id))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch notification: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_update_selected_notification_seen_states() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .patch(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "notificationIds": [first_notification_id, second_notification_id],
            "seen": true,
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch selected notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, first_notification_id).await);
    assert!(notification_seen(&token, second_notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_update_all_notification_seen_states() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, true).await;

    let response = reqwest::Client::new()
        .patch(format!("{}/all", notifications_path()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch all notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, first_notification_id).await);
    assert!(notification_seen(&token, second_notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_delete_one_notification() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let deleted_notification_id = seed_notification(user_id, false).await;
    let retained_notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .delete(notification_path(deleted_notification_id))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete notification: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    let notification_ids = listed_notification_ids(&token).await;
    assert!(!notification_ids.contains(&deleted_notification_id.to_string()));
    assert!(notification_ids.contains(&retained_notification_id.to_string()));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_delete_all_notifications() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let _ = seed_notification(user_id, false).await;
    let _ = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .delete(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete all notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(listed_notification_ids(&token).await.is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_return_not_found_for_missing_notification_mutations() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let notification_id = Uuid::new_v4();
    let client = reqwest::Client::new();

    let update = client
        .patch(notification_path(notification_id))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch missing notification: {error}"));
    let (update_status, update_body) = json_response(update).await;
    assert_problem(
        update_status,
        &update_body,
        reqwest::StatusCode::NOT_FOUND,
        "NOTIFICATION_NOT_FOUND",
    );

    let delete = client
        .delete(notification_path(notification_id))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete missing notification: {error}"));
    let (delete_status, delete_body) = json_response(delete).await;
    assert_problem(
        delete_status,
        &delete_body,
        reqwest::StatusCode::NOT_FOUND,
        "NOTIFICATION_NOT_FOUND",
    );
}

fn notifications_path() -> String {
    format!("{}/api/v1/me/notifications", AURA_API.base_url())
}

fn notification_path(notification_id: Uuid) -> String {
    format!("{}/{}", notifications_path(), notification_id)
}

async fn notification_token(user_id: UserId) -> RawAccessToken {
    seed_access_token_for(user_id, HashSet::new()).await
}

async fn seed_notification(user_id: UserId, seen: bool) -> Uuid {
    let notification_id = Uuid::new_v4();
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id,
            user_id,
            kind,
            partner_shop_application_id,
            payload,
            seen
        ) VALUES ($1, $2, 'PARTNER_APPLICATION_APPROVED', $3, $4, $5)
        "#,
    )
    .bind(notification_id)
    .bind(uuid::Uuid::from(user_id))
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({
        "type": "PARTNER_APPLICATION",
        "snapshot": { "shop_name": "Acceptance Shop", "image": null },
    }))
    .bind(seen)
    .execute(&pool)
    .await
    {
        panic!("failed to seed notification: {error}");
    }
    notification_id
}

async fn notification_seen(token: &RawAccessToken, notification_id: Uuid) -> bool {
    let notification = listed_notifications(token)
        .await
        .into_iter()
        .find(|notification| notification["notificationId"] == notification_id.to_string());
    match notification.and_then(|notification| notification["seen"].as_bool()) {
        Some(seen) => seen,
        None => panic!("notification {notification_id} was not listed with a seen state"),
    }
}

async fn listed_notification_ids(token: &RawAccessToken) -> Vec<String> {
    listed_notifications(token)
        .await
        .into_iter()
        .filter_map(|notification| {
            notification["notificationId"]
                .as_str()
                .map(ToOwned::to_owned)
        })
        .collect()
}

async fn listed_notifications(token: &RawAccessToken) -> Vec<Value> {
    let response = reqwest::Client::new()
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "100")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify listed notifications: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    match body["items"].as_array() {
        Some(items) => items.clone(),
        None => panic!("notification list response is missing items"),
    }
}
