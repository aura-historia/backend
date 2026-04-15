mod signature;
mod types;

use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::user_id::UserId;
use http::StatusCode;
use lambda_runtime::LambdaEvent;
use tracing::{error, info, warn};
use user::core::tier::UserTier;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;

pub use signature::verify_signature;
pub use types::*;

#[tracing::instrument(
    skip(event, service, webhook_secret),
    fields(requestId = %event.context.request_id),
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
    webhook_secret: &str,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    let request = &event.payload;

    let raw_body = match &request.body {
        Some(body) => body,
        None => {
            warn!("Received webhook with empty body.");
            return Ok(response(StatusCode::BAD_REQUEST, "Missing request body"));
        }
    };

    let signature = request
        .headers
        .get("x-signature")
        .and_then(|v| v.to_str().ok());

    let signature = match signature {
        Some(sig) => sig,
        None => {
            warn!("Received webhook without X-Signature header.");
            return Ok(response(StatusCode::UNAUTHORIZED, "Missing signature"));
        }
    };

    if !verify_signature(raw_body, signature, webhook_secret) {
        warn!("Webhook signature verification failed.");
        return Ok(response(StatusCode::UNAUTHORIZED, "Invalid signature"));
    }

    let webhook: LemonSqueezyWebhook = match serde_json::from_str(raw_body) {
        Ok(wh) => wh,
        Err(err) => {
            error!(error = %err, "Failed to parse webhook payload.");
            return Ok(response(StatusCode::BAD_REQUEST, "Invalid payload"));
        }
    };

    let event_name = &webhook.meta.event_name;
    info!(event = %event_name, "Processing Lemon Squeezy webhook event.");

    let user_id = webhook.meta.custom_data.as_ref().and_then(|cd| {
        cd.user_id
            .as_ref()
            .and_then(|uid| UserId::try_from(uid.as_str()).ok())
    });

    match event_name {
        WebhookEventName::SubscriptionCreated
        | WebhookEventName::SubscriptionUpdated
        | WebhookEventName::SubscriptionResumed
        | WebhookEventName::SubscriptionUnpaused => {
            handle_subscription_active(service, user_id.as_ref(), &webhook).await
        }
        WebhookEventName::SubscriptionCancelled | WebhookEventName::SubscriptionExpired => {
            handle_subscription_inactive(service, user_id.as_ref()).await
        }
        WebhookEventName::SubscriptionPaused => {
            handle_subscription_inactive(service, user_id.as_ref()).await
        }
        WebhookEventName::SubscriptionPaymentSuccess
        | WebhookEventName::SubscriptionPaymentRecovered => {
            handle_subscription_active(service, user_id.as_ref(), &webhook).await
        }
        WebhookEventName::SubscriptionPaymentFailed
        | WebhookEventName::SubscriptionPaymentRefunded => {
            info!(event = %event_name, "Subscription payment event acknowledged.");
            Ok(response(StatusCode::OK, "Event acknowledged"))
        }
        WebhookEventName::OrderCreated
        | WebhookEventName::OrderRefunded
        | WebhookEventName::CustomerUpdated
        | WebhookEventName::LicenseKeyCreated
        | WebhookEventName::LicenseKeyUpdated => {
            info!(event = %event_name, "Non-subscription event acknowledged.");
            Ok(response(StatusCode::OK, "Event acknowledged"))
        }
        WebhookEventName::Unknown(name) => {
            warn!(event = %name, "Received unknown Lemon Squeezy event.");
            Ok(response(StatusCode::OK, "Unknown event acknowledged"))
        }
    }
}

async fn handle_subscription_active(
    service: &impl UserService,
    user_id: Option<&UserId>,
    webhook: &LemonSqueezyWebhook,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    let user_id = match user_id {
        Some(id) => id,
        None => {
            warn!("Subscription event received without valid userId in custom_data.");
            return Ok(response(StatusCode::OK, "No user_id, event skipped"));
        }
    };

    let status = webhook
        .data
        .attributes
        .get("status")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            serde_json::from_value::<SubscriptionStatus>(serde_json::Value::String(s.to_owned()))
                .ok()
        });

    let tier = match status {
        Some(SubscriptionStatus::Active) | Some(SubscriptionStatus::OnTrial) => UserTier::Pro,
        Some(SubscriptionStatus::Paused)
        | Some(SubscriptionStatus::PastDue)
        | Some(SubscriptionStatus::Unpaid)
        | Some(SubscriptionStatus::Cancelled)
        | Some(SubscriptionStatus::Expired) => UserTier::Free,
        None => {
            warn!(userId = %user_id, "Subscription event without recognizable status, defaulting to Pro.");
            UserTier::Pro
        }
    };

    let cmd = UpdateUserCommand {
        tier: Some(tier),
        ..Default::default()
    };

    match service.update_user(user_id, cmd).await {
        Ok(user) => {
            info!(userId = %user.user_id, tier = ?user.tier, "Updated user tier.");
            Ok(response(StatusCode::OK, "User tier updated"))
        }
        Err(err) => {
            error!(userId = %user_id, error = %err, "Failed to update user tier.");
            Ok(response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to update user",
            ))
        }
    }
}

async fn handle_subscription_inactive(
    service: &impl UserService,
    user_id: Option<&UserId>,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    let user_id = match user_id {
        Some(id) => id,
        None => {
            warn!("Subscription inactive event received without valid userId in custom_data.");
            return Ok(response(StatusCode::OK, "No user_id, event skipped"));
        }
    };

    let cmd = UpdateUserCommand {
        tier: Some(UserTier::Free),
        ..Default::default()
    };

    match service.update_user(user_id, cmd).await {
        Ok(user) => {
            info!(userId = %user.user_id, tier = ?user.tier, "Downgraded user tier to Free.");
            Ok(response(StatusCode::OK, "User tier downgraded"))
        }
        Err(err) => {
            error!(userId = %user_id, error = %err, "Failed to downgrade user tier.");
            Ok(response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to update user",
            ))
        }
    }
}

fn response(status: StatusCode, body: &str) -> ApiGatewayV2httpResponse {
    ApiGatewayV2HttpResponseBuilder::plain(status.as_u16() as i64)
        .body(body.to_owned())
        .build()
}
