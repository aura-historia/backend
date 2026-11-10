use crate::OperationError;
use notification_service::ports::notification_list_reader::NotificationListReadError;
use notification_service::use_cases::{
    commands::{
        delete_notification::DeleteNotificationError,
        delete_notifications::DeleteNotificationsError,
        update_all_notifications_seen::UpdateAllNotificationsSeenError,
        update_notification_seen::UpdateNotificationSeenError,
        update_notifications_seen::UpdateNotificationsSeenError,
    },
    queries::list_notifications::ListNotificationsError,
};

pub(super) fn list(error: ListNotificationsError) -> OperationError {
    match error {
        ListNotificationsError::AuthenticatedActorRequired | ListNotificationsError::Forbidden => {
            OperationError::Forbidden
        }
        ListNotificationsError::ReadFailed(NotificationListReadError::InvalidReadModel {
            source,
        }) => OperationError::Internal(source),
        ListNotificationsError::ReadFailed(error) => OperationError::Unavailable(Box::new(error)),
    }
}
pub(super) fn update_one(error: UpdateNotificationSeenError) -> OperationError {
    match error {
        UpdateNotificationSeenError::AuthenticatedActorRequired
        | UpdateNotificationSeenError::Forbidden => OperationError::Forbidden,
        UpdateNotificationSeenError::NotFound => {
            OperationError::NotFound("Notification not found: check the notificationId.")
        }
        UpdateNotificationSeenError::UpdateFailed(error) => {
            OperationError::Unavailable(Box::new(error))
        }
    }
}
pub(super) fn update_selected(error: UpdateNotificationsSeenError) -> OperationError {
    match error {
        UpdateNotificationsSeenError::AuthenticatedActorRequired
        | UpdateNotificationsSeenError::Forbidden => OperationError::Forbidden,
        UpdateNotificationsSeenError::EmptyNotificationIds => OperationError::InvalidInput,
        UpdateNotificationsSeenError::UpdateFailed(error) => {
            OperationError::Unavailable(Box::new(error))
        }
    }
}
pub(super) fn update_all(error: UpdateAllNotificationsSeenError) -> OperationError {
    match error {
        UpdateAllNotificationsSeenError::AuthenticatedActorRequired
        | UpdateAllNotificationsSeenError::Forbidden => OperationError::Forbidden,
        UpdateAllNotificationsSeenError::UpdateFailed(error) => {
            OperationError::Unavailable(Box::new(error))
        }
    }
}
pub(super) fn delete_one(error: DeleteNotificationError) -> OperationError {
    match error {
        DeleteNotificationError::AuthenticatedActorRequired
        | DeleteNotificationError::Forbidden => OperationError::Forbidden,
        DeleteNotificationError::NotFound => {
            OperationError::NotFound("Notification not found: check the notificationId.")
        }
        DeleteNotificationError::DeleteFailed(error) => {
            OperationError::Unavailable(Box::new(error))
        }
    }
}
pub(super) fn delete_all(error: DeleteNotificationsError) -> OperationError {
    match error {
        DeleteNotificationsError::AuthenticatedActorRequired
        | DeleteNotificationsError::Forbidden => OperationError::Forbidden,
        DeleteNotificationsError::DeleteFailed(error) => {
            OperationError::Unavailable(Box::new(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_name_the_notification_that_was_not_found() {
        for texts in [
            update_one(UpdateNotificationSeenError::NotFound).tool_error_texts(),
            delete_one(DeleteNotificationError::NotFound).tool_error_texts(),
        ] {
            assert_eq!(
                vec![
                    "NOT_FOUND",
                    "Notification not found: check the notificationId."
                ],
                texts
            );
        }
    }
}
