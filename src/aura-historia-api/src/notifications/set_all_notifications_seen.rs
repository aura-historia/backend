use super::types::{UpdateNotificationSeenData, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::NotificationsState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::update_all_notifications_seen::UpdateAllNotificationsSeenCommand;

pub(super) async fn update_all_notifications(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let data: UpdateNotificationSeenData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };

    match state
        .update_all_notifications_seen
        .execute(
            &context,
            UpdateAllNotificationsSeenCommand { seen: data.seen },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
