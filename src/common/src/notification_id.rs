crate::uuid_v7_newtype!(NotificationId);

impl From<NotificationId> for uuid::Uuid {
    fn from(id: NotificationId) -> Self {
        id.0
    }
}
