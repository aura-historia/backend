use crate::ports::{
    external_delivery_plan_reader::{
        ExternalDeliveryPlanReader, ExternalDeliveryPlanReaderFactory, NotificationDeliveryPlan,
    },
    notification_creator::{
        ExternalDeliveryRequest, NewNotification, NotificationCreationError,
        NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
    },
    notification_delivery_intent_repository::{
        NewNotificationDeliveryIntent, NotificationDeliveryIntentRepository,
        NotificationDeliveryIntentRepositoryFactory,
    },
    notification_repository::{NotificationRepository, NotificationRepositoryFactory},
};
use common::{error::boxed::box_error, notification_id::NotificationId};
use notification_core::notification_delivery_id::NotificationDeliveryId;

#[derive(Debug, Clone, Copy, Default)]
pub struct NotificationCreationCoordinatorFactory<R, P, D> {
    notifications: R,
    plans: P,
    deliveries: D,
}

impl<R, P, D> NotificationCreationCoordinatorFactory<R, P, D> {
    pub fn new(notifications: R, plans: P, deliveries: D) -> Self {
        Self {
            notifications,
            plans,
            deliveries,
        }
    }
}

pub struct NotificationCreationCoordinator<'tx, Tx, R, P, D> {
    tx: &'tx mut Tx,
    notifications: &'tx R,
    plans: &'tx P,
    deliveries: &'tx D,
}

impl<Tx, R, P, D> NotificationCreatorFactory<Tx> for NotificationCreationCoordinatorFactory<R, P, D>
where
    R: NotificationRepositoryFactory<Tx>,
    P: ExternalDeliveryPlanReaderFactory<Tx>,
    D: NotificationDeliveryIntentRepositoryFactory<Tx>,
    Tx: Send,
{
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl NotificationCreator + 'tx {
        NotificationCreationCoordinator {
            tx,
            notifications: &self.notifications,
            plans: &self.plans,
            deliveries: &self.deliveries,
        }
    }
}

#[async_trait::async_trait]
impl<Tx, R, P, D> NotificationCreator for NotificationCreationCoordinator<'_, Tx, R, P, D>
where
    R: NotificationRepositoryFactory<Tx>,
    P: ExternalDeliveryPlanReaderFactory<Tx>,
    D: NotificationDeliveryIntentRepositoryFactory<Tx>,
    Tx: Send,
{
    async fn create_many(
        &mut self,
        notifications: &[NewNotification],
    ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
        let outcomes = self
            .notifications
            .in_transaction(self.tx)
            .insert_many(notifications)
            .await?;
        if outcomes.len() != notifications.len() {
            return Err(creation_error(
                "notification repository returned incomplete outcomes",
            ));
        }

        let mut deliveries = Vec::new();
        for (notification, outcome) in notifications.iter().zip(&outcomes) {
            if notification.external_delivery != ExternalDeliveryRequest::Requested
                || !matches!(outcome, NotificationCreationOutcome::Inserted { .. })
            {
                continue;
            }
            let plans = self
                .plans
                .in_transaction(self.tx)
                .plans_for(
                    notification.notification.user_id(),
                    notification.notification.kind(),
                )
                .await
                .map_err(|error| NotificationCreationError::CreateFailed {
                    source: box_error(error),
                })?;
            deliveries.extend(deliveries_for(
                notification.notification.notification_id(),
                plans,
            ));
        }
        self.deliveries
            .in_transaction(self.tx)
            .insert_many(&deliveries)
            .await?;
        Ok(outcomes)
    }
}

fn deliveries_for(
    notification_id: NotificationId,
    plans: Vec<NotificationDeliveryPlan>,
) -> impl Iterator<Item = NewNotificationDeliveryIntent> {
    plans
        .into_iter()
        .map(move |plan| NewNotificationDeliveryIntent {
            notification_delivery_id: NotificationDeliveryId::new(),
            notification_id,
            plan,
        })
}

fn creation_error(message: &'static str) -> NotificationCreationError {
    NotificationCreationError::CreateFailed {
        source: box_error(std::io::Error::other(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notification_core::notification_delivery::{
        NotificationDeliveryChannel, NotificationDeliveryTargetKey,
    };

    #[test]
    fn should_expand_each_target_into_an_independent_delivery_intent()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_id = NotificationId::new();
        let plans = vec![
            NotificationDeliveryPlan {
                channel: NotificationDeliveryChannel::Email,
                target_key: NotificationDeliveryTargetKey::primary(),
            },
            NotificationDeliveryPlan {
                channel: NotificationDeliveryChannel::Email,
                target_key: NotificationDeliveryTargetKey::try_from("SECONDARY".to_owned())?,
            },
        ];

        let deliveries = deliveries_for(notification_id, plans).collect::<Vec<_>>();

        assert_eq!(2, deliveries.len());
        assert_eq!(notification_id, deliveries[0].notification_id);
        assert_eq!(notification_id, deliveries[1].notification_id);
        assert_ne!(
            deliveries[0].notification_delivery_id,
            deliveries[1].notification_delivery_id
        );
        assert_ne!(deliveries[0].plan.target_key, deliveries[1].plan.target_key);
        Ok(())
    }
}
