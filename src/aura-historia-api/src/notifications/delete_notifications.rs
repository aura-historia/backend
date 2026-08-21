use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::NotificationsState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::delete_notifications::DeleteNotificationsCommand;

pub(super) async fn delete_notifications(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .delete_notifications
        .execute(&context, DeleteNotificationsCommand)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
