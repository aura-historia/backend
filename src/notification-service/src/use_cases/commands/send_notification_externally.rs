use crate::ports::{
    external_notification_sender::{
        ExternalNotificationMessage, ExternalNotificationSendError, ExternalNotificationSender,
    },
    notification_recipient_reader::{NotificationRecipientReadError, NotificationRecipientReader},
    notification_repository::{NotificationRepository, NotificationRepositoryError},
};
use domain_primitives::event_id::EventId;
use notification_core::{notification::Notification, notification_type::NotificationType};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendNotificationExternallyCommand {
    pub user_id: UserId,
    pub origin_event_id: EventId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendNotificationExternallyResult {
    pub notification: Notification,
    pub sent: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SendNotificationExternallyError {
    #[error("notification not found")]
    NotFound,
    #[error("notification lookup failed")]
    LookupFailed(#[source] NotificationRepositoryError),
    #[error("notification recipient not found")]
    RecipientNotFound,
    #[error("notification recipient unavailable")]
    RecipientUnavailable(#[source] NotificationRecipientReadError),
    #[error("external notification send failed")]
    SendFailed(#[source] ExternalNotificationSendError),
    #[error("notification update failed")]
    UpdateFailed(#[source] NotificationRepositoryError),
}

#[async_trait::async_trait]
pub trait SendNotificationExternallyUseCase: Send + Sync {
    async fn execute(
        &self,
        command: SendNotificationExternallyCommand,
    ) -> Result<SendNotificationExternallyResult, SendNotificationExternallyError>;
}

pub struct SendNotificationExternallyHandler<R, U, S> {
    repository: R,
    recipients: U,
    sender: S,
}

impl<R, U, S> SendNotificationExternallyHandler<R, U, S> {
    pub fn new(repository: R, recipients: U, sender: S) -> Self {
        Self {
            repository,
            recipients,
            sender,
        }
    }
}

#[async_trait::async_trait]
impl<R, U, S> SendNotificationExternallyUseCase for SendNotificationExternallyHandler<R, U, S>
where
    R: NotificationRepository,
    U: NotificationRecipientReader,
    S: ExternalNotificationSender,
{
    async fn execute(
        &self,
        command: SendNotificationExternallyCommand,
    ) -> Result<SendNotificationExternallyResult, SendNotificationExternallyError> {
        let mut notification = self
            .repository
            .find_by_origin_event_id(&command.user_id, &command.origin_event_id)
            .await
            .map_err(SendNotificationExternallyError::LookupFailed)?
            .ok_or(SendNotificationExternallyError::NotFound)?;

        if !notification.external() || notification.notification_type().is_some() {
            return Ok(SendNotificationExternallyResult {
                notification,
                sent: false,
            });
        }

        let recipient = self
            .recipients
            .find_recipient(&command.user_id)
            .await
            .map_err(SendNotificationExternallyError::RecipientUnavailable)?
            .ok_or(SendNotificationExternallyError::RecipientNotFound)?;
        let payload = notification
            .notification_payload()
            .clone()
            .localized(&recipient.currency, &recipient.languages);
        self.sender
            .send(ExternalNotificationMessage { recipient, payload })
            .await
            .map_err(SendNotificationExternallyError::SendFailed)?;
        notification.mark_sent_as(NotificationType::Email);
        let notification = self
            .repository
            .update(&notification)
            .await
            .map_err(SendNotificationExternallyError::UpdateFailed)?;
        Ok(SendNotificationExternallyResult {
            notification,
            sent: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::notification_recipient_reader::NotificationRecipient;
    use application::error::{BoxError, box_error};
    use localization::Language;
    use money::Currency;
    use notification_core::{
        notification::{
            NotificationPartnerApplicationPayload, NotificationPayload, RehydratedNotificationState,
        },
        notification_id::NotificationId,
    };
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeGateway {
        notification: Arc<Mutex<Option<Notification>>>,
        recipient: Arc<Mutex<Option<NotificationRecipient>>>,
        sent: Arc<Mutex<usize>>,
    }

    fn payload() -> NotificationPayload {
        NotificationPayload::PartnerApplication {
            shop_name: ShopName::from("test shop"),
            image: None,
            partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                partner_application_id: PartnerShopApplicationId::new(),
            },
        }
    }

    fn notification_for(
        user_id: UserId,
        origin_event_id: EventId,
        notification_type: Option<NotificationType>,
        external: bool,
    ) -> Notification {
        Notification::rehydrate(RehydratedNotificationState {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type,
            notification_payload: payload(),
            seen: false,
            external,
        })
    }

    #[async_trait::async_trait]
    impl NotificationRepository for FakeGateway {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }

        async fn find_by_origin_event_id(
            &self,
            user_id: &UserId,
            origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            Ok(self
                .notification
                .lock()
                .unwrap()
                .clone()
                .filter(|notification| {
                    notification.user_id() == *user_id
                        && notification.origin_event_id() == *origin_event_id
                }))
        }

        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            *self.notification.lock().unwrap() = Some(notification.clone());
            Ok(notification.clone())
        }
    }

    #[async_trait::async_trait]
    impl NotificationRecipientReader for FakeGateway {
        async fn find_recipient(
            &self,
            _user_id: &UserId,
        ) -> Result<Option<NotificationRecipient>, NotificationRecipientReadError> {
            Ok(self.recipient.lock().unwrap().clone())
        }
    }

    #[async_trait::async_trait]
    impl ExternalNotificationSender for FakeGateway {
        async fn send(
            &self,
            _message: ExternalNotificationMessage,
        ) -> Result<(), ExternalNotificationSendError> {
            *self.sent.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_send_external_notification_and_mark_sent() {
        let gateway = FakeGateway::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *gateway.notification.lock().unwrap() =
            Some(notification_for(user_id, origin_event_id, None, true));
        *gateway.recipient.lock().unwrap() = Some(NotificationRecipient {
            user_id,
            email: "test@example.com".into(),
            first_name: Some("Test".into()),
            languages: vec![Language::En],
            currency: Currency::Eur,
        });

        let result = SendNotificationExternallyHandler::new(
            gateway.clone(),
            gateway.clone(),
            gateway.clone(),
        )
        .execute(SendNotificationExternallyCommand {
            user_id,
            origin_event_id,
        })
        .await
        .expect("send should succeed");

        assert!(result.sent);
        assert_eq!(
            Some(NotificationType::Email),
            result.notification.notification_type()
        );
        assert_eq!(1, *gateway.sent.lock().unwrap());
    }

    #[tokio::test]
    async fn should_skip_external_send_when_not_external() {
        let gateway = FakeGateway::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *gateway.notification.lock().unwrap() =
            Some(notification_for(user_id, origin_event_id, None, false));

        let result = SendNotificationExternallyHandler::new(
            gateway.clone(),
            gateway.clone(),
            gateway.clone(),
        )
        .execute(SendNotificationExternallyCommand {
            user_id,
            origin_event_id,
        })
        .await
        .expect("send should be skipped");

        assert!(!result.sent);
        assert_eq!(0, *gateway.sent.lock().unwrap());
    }

    #[derive(Clone, Default)]
    struct FailingSender(FakeGateway);

    #[async_trait::async_trait]
    impl NotificationRepository for FailingSender {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            self.0.insert(notification).await
        }
        async fn find_by_origin_event_id(
            &self,
            user_id: &UserId,
            origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            self.0
                .find_by_origin_event_id(user_id, origin_event_id)
                .await
        }
        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            self.0.update(notification).await
        }
    }

    #[async_trait::async_trait]
    impl NotificationRecipientReader for FailingSender {
        async fn find_recipient(
            &self,
            user_id: &UserId,
        ) -> Result<Option<NotificationRecipient>, NotificationRecipientReadError> {
            self.0.find_recipient(user_id).await
        }
    }

    #[async_trait::async_trait]
    impl ExternalNotificationSender for FailingSender {
        async fn send(
            &self,
            _message: ExternalNotificationMessage,
        ) -> Result<(), ExternalNotificationSendError> {
            let source: BoxError = box_error(std::io::Error::other("boom"));
            Err(ExternalNotificationSendError::SendFailed { source })
        }
    }

    #[tokio::test]
    async fn should_fail_external_send_when_sender_fails() {
        let gateway = FailingSender(FakeGateway::default());
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *gateway.0.notification.lock().unwrap() =
            Some(notification_for(user_id, origin_event_id, None, true));
        *gateway.0.recipient.lock().unwrap() = Some(NotificationRecipient {
            user_id,
            email: "test@example.com".into(),
            first_name: None,
            languages: vec![Language::En],
            currency: Currency::Eur,
        });

        let result =
            SendNotificationExternallyHandler::new(gateway.clone(), gateway.clone(), gateway)
                .execute(SendNotificationExternallyCommand {
                    user_id,
                    origin_event_id,
                })
                .await;

        assert!(matches!(
            result,
            Err(SendNotificationExternallyError::SendFailed(_))
        ));
    }
}
