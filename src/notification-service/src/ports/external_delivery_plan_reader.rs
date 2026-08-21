use application::error::BoxError;
use notification_core::{
    notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    notification_kind::NotificationKind,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryPlan {
    pub channel: NotificationDeliveryChannel,
    pub target_key: NotificationDeliveryTargetKey,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalDeliveryPlanReadError {
    #[error("external delivery plan read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ExternalDeliveryPlanReader: Send {
    async fn plans_for(
        &mut self,
        user_id: UserId,
        notification_kind: NotificationKind,
    ) -> Result<Vec<NotificationDeliveryPlan>, ExternalDeliveryPlanReadError>;
}

pub trait ExternalDeliveryPlanReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ExternalDeliveryPlanReader + 'tx;
}
