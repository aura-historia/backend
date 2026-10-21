use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, INVALID_OBJECT_ID};
use crate::state::NotificationsState;
use aura_historia_contracts::notifications::{
    InvalidNotificationCursor, ListNotificationsQuery, NotificationsPageData,
};
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};

pub(super) async fn list_notifications(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query = match crate::wire::from_query_str::<ListNotificationsQuery>(
        raw_query.as_deref().unwrap_or_default(),
    ) {
        Ok(query) => query,
        Err(error) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    let request = match query.into_request() {
        Ok(request) => request,
        Err(error) => return cursor_error(error).into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state.list_notifications.execute(&context, request).await {
        Ok(result) => match NotificationsPageData::try_from(result) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(error) => ApiError::internal_server_error(BAD_QUERY_PARAMETER_VALUE)
                .with_detail("Notification cursor failed internally.")
                .with_cause(Box::new(error))
                .into_response(),
        },
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn cursor_error(error: InvalidNotificationCursor) -> ApiError {
    let code = if matches!(error, InvalidNotificationCursor::NotificationId(_)) {
        INVALID_OBJECT_ID
    } else {
        BAD_QUERY_PARAMETER_VALUE
    };
    ApiError::bad_request(code)
        .with_query_field("searchAfter")
        .with_detail(error.to_string())
        .with_cause(Box::new(error))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use aura_historia_contracts::notifications::parse_notification_cursor;
    use notification_core::notification_id::NotificationId;

    #[test]
    fn should_parse_notification_cursor_with_type_id() {
        let created = "2026-08-19T12:00:00Z";
        let notification_id = NotificationId::new();
        let cursor = parse_notification_cursor(&format!(r#"["{created}","{notification_id}"]"#));
        assert!(matches!(cursor, Ok(cursor) if cursor.notification_id == notification_id));
    }

    #[test]
    fn should_reject_non_array_notification_cursor() {
        assert!(parse_notification_cursor(r#""not-a-cursor""#).is_err());
    }
}
