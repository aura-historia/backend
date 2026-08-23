domain_primitives::uuid_v7_newtype!(NotificationDeliveryId);

impl From<NotificationDeliveryId> for uuid::Uuid {
    fn from(id: NotificationDeliveryId) -> Self {
        id.0
    }
}
