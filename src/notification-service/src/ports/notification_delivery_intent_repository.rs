use crate::ports::{
    external_delivery_plan_reader::NotificationDeliveryPlan,
    notification_creator::NotificationCreationError,
};
use notification_core::{
    notification_delivery_id::NotificationDeliveryId, notification_id::NotificationId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotificationDeliveryIntent {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
    pub plan: NotificationDeliveryPlan,
}

#[async_trait::async_trait]
pub trait NotificationDeliveryIntentRepository: Send {
    async fn insert_many(
        &mut self,
        deliveries: &[NewNotificationDeliveryIntent],
    ) -> Result<(), NotificationCreationError>;
}

pub trait NotificationDeliveryIntentRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl NotificationDeliveryIntentRepository + 'tx;
}
