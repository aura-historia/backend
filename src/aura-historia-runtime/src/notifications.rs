use notification_postgres::{
    SqlxNotificationDeleter, SqlxNotificationListReader, SqlxNotificationSeenWriter,
};
use notification_service::use_cases::{
    commands::{
        delete_notification::{DeleteNotificationHandler, DeleteNotificationUseCase},
        delete_notifications::{DeleteNotificationsHandler, DeleteNotificationsUseCase},
        update_all_notifications_seen::{
            UpdateAllNotificationsSeenHandler, UpdateAllNotificationsSeenUseCase,
        },
        update_notification_seen::{UpdateNotificationSeenHandler, UpdateNotificationSeenUseCase},
        update_notifications_seen::{
            UpdateNotificationsSeenHandler, UpdateNotificationsSeenUseCase,
        },
    },
    queries::list_notifications::{ListNotificationsHandler, ListNotificationsUseCase},
};
use sqlx::PgPool;
use std::sync::Arc;

/// The notification inbox only; no delivery or arbitrary messaging capability.
#[derive(Clone)]
pub struct Notifications {
    pub list: Arc<dyn ListNotificationsUseCase>,
    pub update_notification_seen: Arc<dyn UpdateNotificationSeenUseCase>,
    pub update_notifications_seen: Arc<dyn UpdateNotificationsSeenUseCase>,
    pub update_all_notifications_seen: Arc<dyn UpdateAllNotificationsSeenUseCase>,
    pub delete_notification: Arc<dyn DeleteNotificationUseCase>,
    pub delete_notifications: Arc<dyn DeleteNotificationsUseCase>,
}

impl Notifications {
    pub(super) fn compose(pool: &PgPool) -> Self {
        Self {
            list: Arc::new(ListNotificationsHandler::new(
                SqlxNotificationListReader::new(pool.clone()),
            )),
            update_notification_seen: Arc::new(UpdateNotificationSeenHandler::new(
                SqlxNotificationSeenWriter::new(pool.clone()),
            )),
            update_notifications_seen: Arc::new(UpdateNotificationsSeenHandler::new(
                SqlxNotificationSeenWriter::new(pool.clone()),
            )),
            update_all_notifications_seen: Arc::new(UpdateAllNotificationsSeenHandler::new(
                SqlxNotificationSeenWriter::new(pool.clone()),
            )),
            delete_notification: Arc::new(DeleteNotificationHandler::new(
                SqlxNotificationDeleter::new(pool.clone()),
            )),
            delete_notifications: Arc::new(DeleteNotificationsHandler::new(
                SqlxNotificationDeleter::new(pool.clone()),
            )),
        }
    }
}
