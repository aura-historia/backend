use crate::ports::external_delivery_plan_reader::{
    ExternalDeliveryPlanReadError, ExternalDeliveryPlanReader, ExternalDeliveryPlanReaderFactory,
    NotificationDeliveryPlan,
};
use common::user_id::UserId;
use notification_core::{
    notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    notification_kind::NotificationKind,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct InitialExternalDeliveryPlanReaderFactory;

struct InitialExternalDeliveryPlanReader;

impl<Tx> ExternalDeliveryPlanReaderFactory<Tx> for InitialExternalDeliveryPlanReaderFactory {
    fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl ExternalDeliveryPlanReader + 'tx {
        InitialExternalDeliveryPlanReader
    }
}

#[async_trait::async_trait]
impl ExternalDeliveryPlanReader for InitialExternalDeliveryPlanReader {
    async fn plans_for(
        &mut self,
        _: UserId,
        _: NotificationKind,
    ) -> Result<Vec<NotificationDeliveryPlan>, ExternalDeliveryPlanReadError> {
        Ok(vec![NotificationDeliveryPlan {
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::external_delivery_plan_reader::ExternalDeliveryPlanReaderFactory;

    #[tokio::test]
    async fn should_plan_email_primary_for_external_delivery()
    -> Result<(), ExternalDeliveryPlanReadError> {
        let mut tx = ();
        let plans = InitialExternalDeliveryPlanReaderFactory
            .in_transaction(&mut tx)
            .plans_for(UserId::new(), NotificationKind::WatchlistPriceChanged)
            .await?;

        assert_eq!(
            vec![NotificationDeliveryPlan {
                channel: NotificationDeliveryChannel::Email,
                target_key: NotificationDeliveryTargetKey::primary(),
            }],
            plans
        );
        Ok(())
    }
}
