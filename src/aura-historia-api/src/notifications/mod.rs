mod delete_notification;
mod delete_notifications;
mod list_notifications;
mod set_all_notifications_seen;
mod set_notification_seen;
mod set_notifications_seen;
mod types;

use crate::state::NotificationsState;
use axum::Router;
use axum::routing::{get, patch};

pub fn router(state: NotificationsState) -> Router {
    Router::new()
        .route(
            "/api/v1/me/notifications",
            get(list_notifications::list_notifications)
                .patch(set_notifications_seen::update_notifications)
                .delete(delete_notifications::delete_notifications),
        )
        .route(
            "/api/v1/me/notifications/all",
            patch(set_all_notifications_seen::update_all_notifications),
        )
        .route(
            "/api/v1/me/notifications/{notification_id}",
            patch(set_notification_seen::update_notification)
                .delete(delete_notification::delete_notification),
        )
        .with_state(state)
}
