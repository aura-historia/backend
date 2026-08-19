use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    notification_id::NotificationId,
};
use notification_core::notification_delivery_id::NotificationDeliveryId;
use notification_service::use_cases::commands::deliver_notification::{
    DeliverNotificationCommand, DeliverNotificationResult, DeliverNotificationUseCase,
};
use std::sync::Arc;
use tracing::{error, info};

pub async fn consume_notification_delivery_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn DeliverNotificationUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let delivery_id = notification_delivery_id(&job)
            .unwrap_or_default()
            .to_owned();
        let use_case_for_retry = Arc::clone(&use_case);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let use_case = Arc::clone(&use_case_for_retry);
            async move { execute_job(use_case, job).await }
        })
        .await;

        match result {
            Ok(()) => info!(
                job_type = "notification_delivery",
                %idempotency_key,
                %ordering_key,
                %delivery_id,
                outcome = "applied",
                "notification delivery job completed"
            ),
            Err(error) => error!(
                job_type = "notification_delivery",
                %idempotency_key,
                %ordering_key,
                %delivery_id,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "notification delivery job failed"
            ),
        }
    }
}

#[tracing::instrument(
    name = "process_notification_delivery_job",
    skip(use_case, job),
    fields(notification_delivery_id = tracing::field::Empty)
)]
async fn execute_job(
    use_case: Arc<dyn DeliverNotificationUseCase>,
    job: DomainJob,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    tracing::Span::current().record(
        "notification_delivery_id",
        tracing::field::display(command.notification_delivery_id),
    );
    let outcome = use_case.execute(command).await.map_err(box_error)?;
    let outcome = match outcome {
        DeliverNotificationResult::Delivered { .. } => "delivered",
        DeliverNotificationResult::DeliveryMissing => "delivery_missing",
        DeliverNotificationResult::AlreadyDelivered => "already_delivered",
        DeliverNotificationResult::AlreadyClaimed => "already_claimed",
        DeliverNotificationResult::SourceMissing => "source_missing",
        DeliverNotificationResult::PermanentlyFailed => "permanently_failed",
    };
    info!(
        job_type = "notification_delivery",
        outcome, "notification delivery processed"
    );
    Ok(())
}

fn notification_delivery_id(job: &DomainJob) -> Option<&str> {
    let DomainJobPayload::NotificationDeliveryCreated(delivery) = &job.payload else {
        return None;
    };
    Some(delivery.notification_delivery_id.as_str())
}

fn command_from_job(
    job: DomainJob,
) -> Result<DeliverNotificationCommand, NotificationDeliveryWorkerError> {
    let DomainJobPayload::NotificationDeliveryCreated(delivery) = job.payload else {
        return Err(NotificationDeliveryWorkerError::UnexpectedJobPayload);
    };
    let notification_delivery_id =
        NotificationDeliveryId::try_from(delivery.notification_delivery_id).map_err(|source| {
            NotificationDeliveryWorkerError::InvalidDeliveryId {
                source: box_error(source),
            }
        })?;
    let notification_id = NotificationId::try_from(delivery.notification_id).map_err(|source| {
        NotificationDeliveryWorkerError::InvalidNotificationId {
            source: box_error(source),
        }
    })?;

    Ok(DeliverNotificationCommand {
        notification_delivery_id,
        notification_id,
    })
}

#[derive(Debug, thiserror::Error)]
enum NotificationDeliveryWorkerError {
    #[error("notification delivery queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("notification delivery job has an invalid delivery id")]
    InvalidDeliveryId {
        #[source]
        source: BoxError,
    },
    #[error("notification delivery job has an invalid notification id")]
    InvalidNotificationId {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QueueConfig,
        cdc::{IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey, WorkerQueue},
        in_memory_queue,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Handler {
        commands: Mutex<Vec<DeliverNotificationCommand>>,
    }

    #[async_trait::async_trait]
    impl DeliverNotificationUseCase for Handler {
        async fn execute(
            &self,
            command: DeliverNotificationCommand,
        ) -> Result<DeliverNotificationResult, notification_service::use_cases::commands::deliver_notification::DeliverNotificationError>{
            self.commands
                .lock()
                .map_err(|_| notification_service::use_cases::commands::deliver_notification::DeliverNotificationError::LeaseLost)?
                .push(command);
            Ok(DeliverNotificationResult::Delivered { attempt_count: 1 })
        }
    }

    #[tokio::test]
    async fn should_map_notification_delivery_job_to_delivery_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(1))?;
        let notification_delivery_id = NotificationDeliveryId::new();
        let notification_id = NotificationId::new();
        sender
            .enqueue(DomainJob {
                target_queue: WorkerQueue::NotificationDelivery,
                idempotency_key: IdempotencyKey::new(format!(
                    "notification-delivery:{notification_delivery_id}"
                )),
                ordering_key: OrderingKey::new(format!("notification:{notification_id}")),
                payload: DomainJobPayload::NotificationDeliveryCreated(
                    NotificationDeliveryCreatedJob {
                        notification_delivery_id: notification_delivery_id.to_string(),
                        notification_id: notification_id.to_string(),
                    },
                ),
            })
            .await?;
        drop(sender);
        let handler = Arc::new(Handler::default());

        consume_notification_delivery_queue(receiver, handler.clone()).await;

        assert_eq!(
            vec![DeliverNotificationCommand {
                notification_delivery_id,
                notification_id,
            }],
            handler
                .commands
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .clone()
        );
        Ok(())
    }
}
