use super::types::notification_id;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::NotificationsState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::delete_notification::DeleteNotificationCommand;
use uuid::Uuid;

pub(super) async fn delete_notification(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    Path(raw_notification_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let notification_id = match Uuid::parse_str(&raw_notification_id) {
        Ok(value) => notification_id(value),
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("notificationId")
                .with_detail("Path parameter 'notificationId' must be a notification UUID.")
                .into_response();
        }
    };

    match state
        .delete_notification
        .execute(&context, DeleteNotificationCommand { notification_id })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
