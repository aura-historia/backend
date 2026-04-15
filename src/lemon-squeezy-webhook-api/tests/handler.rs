use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::user_id::UserId;
use hmac::{Hmac, KeyInit, Mac};
use http::HeaderMap;
use lambda_runtime::{Context, LambdaEvent};
use lemon_squeezy_webhook_api::handler;
use sha2::Sha256;
use user::core::tier::UserTier;
use user::core::user::User;
use user::service::command::UpdateUserCommand;
use user::service::user_service::MockUserService;

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_SECRET: &str = "test-webhook-secret-for-integration-tests";

fn sign(payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    Mac::update(&mut mac, payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn make_event(body: Option<&str>, signature: Option<&str>) -> LambdaEvent<ApiGatewayV2httpRequest> {
    let mut headers = HeaderMap::new();
    if let Some(sig) = signature {
        headers.insert("x-signature", sig.parse().unwrap());
    }

    let mut request = ApiGatewayV2httpRequest::default();
    request.body = body.map(|b| b.to_owned());
    request.headers = headers;

    LambdaEvent {
        payload: request,
        context: Context::default(),
    }
}

fn make_signed_event(body: &str) -> LambdaEvent<ApiGatewayV2httpRequest> {
    let sig = sign(body);
    make_event(Some(body), Some(&sig))
}

fn webhook_payload(event_name: &str, status: &str, user_id: &str) -> String {
    serde_json::json!({
        "meta": {
            "event_name": event_name,
            "test_mode": false,
            "custom_data": {
                "user_id": user_id
            }
        },
        "data": {
            "id": "123",
            "type": "subscriptions",
            "attributes": {
                "status": status,
                "store_id": 1
            }
        }
    })
    .to_string()
}

fn webhook_payload_without_user_id(event_name: &str, status: &str) -> String {
    serde_json::json!({
        "meta": {
            "event_name": event_name,
            "test_mode": false
        },
        "data": {
            "id": "123",
            "type": "subscriptions",
            "attributes": {
                "status": status,
                "store_id": 1
            }
        }
    })
    .to_string()
}

fn webhook_payload_non_subscription(event_name: &str) -> String {
    serde_json::json!({
        "meta": {
            "event_name": event_name,
            "test_mode": false
        },
        "data": {
            "id": "456",
            "type": "orders",
            "attributes": {
                "total": 2999
            }
        }
    })
    .to_string()
}

fn mock_user() -> User {
    use fake::Fake;
    fake::Faker.fake()
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_return_401_when_signature_missing() {
    let service = MockUserService::default();
    let body = webhook_payload(
        "subscription_created",
        "active",
        "550e8400-e29b-41d4-a716-446655440000",
    );
    let event = make_event(Some(&body), None);

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 401);
}

#[tokio::test]
async fn should_return_401_when_signature_invalid() {
    let service = MockUserService::default();
    let body = webhook_payload(
        "subscription_created",
        "active",
        "550e8400-e29b-41d4-a716-446655440000",
    );
    let event = make_event(
        Some(&body),
        Some("deadbeef00000000000000000000000000000000000000000000000000000000"),
    );

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 401);
}

#[tokio::test]
async fn should_return_400_when_body_missing() {
    let service = MockUserService::default();
    let event = make_event(None, None);

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 400);
}

#[tokio::test]
async fn should_return_400_when_payload_invalid_json() {
    let service = MockUserService::default();
    let body = "this is not valid json";
    let event = make_signed_event(body);

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 400);
}

// ---------------------------------------------------------------------------
// Subscription created — active status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_created_with_active_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_created", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription created — on_trial status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_created_with_on_trial_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_created", "on_trial", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription updated — active status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_updated_with_active_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription updated — paused status (should set via handle_subscription_active
// but status-based logic maps paused to Free)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_updated_with_paused_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "paused", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription updated — past_due status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_updated_with_past_due_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "past_due", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription updated — unpaid status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_updated_with_unpaid_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "unpaid", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription cancelled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_cancelled() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_cancelled", "cancelled", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription expired
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_expired() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_expired", "expired", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription paused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_paused() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_paused", "paused", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription resumed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_resumed() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_resumed", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription unpaused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_unpaused() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_unpaused", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription payment success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_payment_success() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_payment_success", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription payment recovered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_upgrade_user_to_pro_when_subscription_payment_recovered() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_payment_recovered", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Pro));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription payment failed — no tier change, just acknowledge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_subscription_payment_failed_without_tier_change() {
    let body = webhook_payload(
        "subscription_payment_failed",
        "past_due",
        "550e8400-e29b-41d4-a716-446655440000",
    );
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription payment refunded — no tier change, just acknowledge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_subscription_payment_refunded_without_tier_change() {
    let body = webhook_payload(
        "subscription_payment_refunded",
        "active",
        "550e8400-e29b-41d4-a716-446655440000",
    );
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Non-subscription events — order_created
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_order_created_event() {
    let body = webhook_payload_non_subscription("order_created");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Non-subscription events — order_refunded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_order_refunded_event() {
    let body = webhook_payload_non_subscription("order_refunded");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Non-subscription events — customer_updated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_customer_updated_event() {
    let body = webhook_payload_non_subscription("customer_updated");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Non-subscription events — license_key_created
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_license_key_created_event() {
    let body = webhook_payload_non_subscription("license_key_created");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Non-subscription events — license_key_updated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_license_key_updated_event() {
    let body = webhook_payload_non_subscription("license_key_updated");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Unknown event — catch-all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_acknowledge_unknown_future_event() {
    let body = webhook_payload_non_subscription("some_future_event_type");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Missing user_id in custom_data — subscription events should gracefully skip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_return_200_when_subscription_created_without_user_id() {
    let body = webhook_payload_without_user_id("subscription_created", "active");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

#[tokio::test]
async fn should_return_200_when_subscription_cancelled_without_user_id() {
    let body = webhook_payload_without_user_id("subscription_cancelled", "cancelled");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

#[tokio::test]
async fn should_return_200_when_subscription_paused_without_user_id() {
    let body = webhook_payload_without_user_id("subscription_paused", "paused");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Invalid user_id — not a valid UUID
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_return_200_when_subscription_event_has_invalid_user_id() {
    let body = webhook_payload("subscription_created", "active", "not-a-valid-uuid");
    let event = make_signed_event(&body);
    let service = MockUserService::default();

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// UserService error — handler returns 500 but doesn't panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_return_500_when_user_service_fails_for_subscription_created() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_created", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, _| {
        let fake_id: UserId = UserId::try_from("550e8400-e29b-41d4-a716-446655440000").unwrap();
        Box::pin(async move {
            Err(user::service::user_service::UserServiceError::UserNotFound(
                fake_id,
            ))
        })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 500);
}

#[tokio::test]
async fn should_return_500_when_user_service_fails_for_subscription_cancelled() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_cancelled", "cancelled", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, _| {
        let fake_id: UserId = UserId::try_from("550e8400-e29b-41d4-a716-446655440000").unwrap();
        Box::pin(async move {
            Err(user::service::user_service::UserServiceError::UserNotFound(
                fake_id,
            ))
        })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 500);
}

// ---------------------------------------------------------------------------
// Subscription updated with expired status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_updated_with_expired_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "expired", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Subscription updated with cancelled status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_downgrade_user_to_free_when_subscription_updated_with_cancelled_status() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_updated", "cancelled", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(cmd.tier, Some(UserTier::Free));
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

// ---------------------------------------------------------------------------
// Verify update_user receives correct UpdateUserCommand defaults
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_only_set_tier_field_in_update_command_for_subscription_created() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_created", "active", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(
            cmd,
            UpdateUserCommand {
                tier: Some(UserTier::Pro),
                ..Default::default()
            }
        );
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}

#[tokio::test]
async fn should_only_set_tier_field_in_update_command_for_subscription_cancelled() {
    let user_id_str = "550e8400-e29b-41d4-a716-446655440000";
    let body = webhook_payload("subscription_cancelled", "cancelled", user_id_str);
    let event = make_signed_event(&body);

    let mut service = MockUserService::default();
    service.expect_update_user().once().returning(|_, cmd| {
        assert_eq!(
            cmd,
            UpdateUserCommand {
                tier: Some(UserTier::Free),
                ..Default::default()
            }
        );
        Box::pin(async { Ok(mock_user()) })
    });

    let response = handler(event, &service, WEBHOOK_SECRET).await.unwrap();
    assert_eq!(response.status_code, 200);
}
