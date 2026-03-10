use crate::{
    core::{
        notification::{LocalizedNotification, Notification},
        notification_id::NotificationId,
    },
    dynamodb::{
        notification_record::NotificationRecord,
        notification_record_update::NotificationRecordUpdate,
        repository::NotificationDynamoDbRepository,
    },
    service::command::{CreateNotificationCommand, UpdateNotificationCommand},
};
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::{
    currency::domain::Currency,
    event_id::EventId,
    language::domain::Language,
    pagination::cursor::{Cursor, CursoredResult},
    user_id::UserId,
};
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum NotificationError {
    #[error("There exists no Notification for user '{0}' with origin-event-id '{1}'.")]
    NotificationNotFound(UserId, EventId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0}")]
    SdkUpdateItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ),
}

#[derive(Debug)]
pub struct CreateNotificationsResult {
    pub unprocessed: Vec<(CreateNotificationCommand, NotificationError)>,
    pub processed: Vec<Notification>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait NotificationService {
    async fn find_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError>;

    async fn create_notification(
        &self,
        origin_event_id: &EventId,
        cmd: CreateNotificationCommand,
    ) -> Result<Notification, NotificationError>;

    async fn create_notifications(
        &self,
        origin_event_id: &EventId,
        cmds: Vec<CreateNotificationCommand>,
    ) -> CreateNotificationsResult;

    async fn update_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: UpdateNotificationCommand,
    ) -> Result<Notification, NotificationError>;

    async fn view_notifications(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        cursor: &Option<Cursor<EventId>>,
    ) -> Result<CursoredResult<LocalizedNotification, EventId>, NotificationError>;
}

pub struct NotificationServiceImpl<'a> {
    notification_repository: &'a (dyn NotificationDynamoDbRepository + Sync),
}

impl<'a> NotificationServiceImpl<'a> {
    pub fn new(notification_repository: &'a (dyn NotificationDynamoDbRepository + Sync)) -> Self {
        Self {
            notification_repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> NotificationService for NotificationServiceImpl<'a> {
    async fn find_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError> {
        let record = self
            .notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        Ok(record.into())
    }

    async fn create_notification(
        &self,
        origin_event_id: &EventId,
        cmd: CreateNotificationCommand,
    ) -> Result<Notification, NotificationError> {
        let now = OffsetDateTime::now_utc();
        let notification = Notification {
            user_id: cmd.user_id,
            origin_event_id: *origin_event_id,
            notification_id: NotificationId::new(),
            notification_payload: cmd.notification_payload,
            seen: false,
            created: now,
            updated: now,
        };
        let record = NotificationRecord::from(notification.clone());
        self.notification_repository
            .put_notification_record(record)
            .await?;

        Ok(notification)
    }

    async fn create_notifications(
        &self,
        origin_event_id: &EventId,
        cmds: Vec<CreateNotificationCommand>,
    ) -> CreateNotificationsResult {
        todo!()
    }

    async fn update_notification(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: UpdateNotificationCommand,
    ) -> Result<Notification, NotificationError> {
        let existing_record = self
            .notification_repository
            .get_notification_record(user_id, origin_event_id)
            .await?
            .ok_or(NotificationError::NotificationNotFound(
                *user_id,
                *origin_event_id,
            ))?;

        if update.is_empty() {
            Ok(existing_record.into())
        } else {
            let record_update = NotificationRecordUpdate {
                seen: update.seen,
                updated: OffsetDateTime::now_utc(),
            };

            let updated_record = self
                .notification_repository
                .update_notification_record(user_id, origin_event_id, record_update)
                .await?
                .ok_or_else(|| {
                    NotificationError::SdkUpdateItemError(SdkError::construction_failure(
                        "Failed parsing DynamoDB UpdateItem Response-Payload",
                    ))
                })?;

            Ok(updated_record.into())
        }
    }

    async fn view_notifications(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        cursor: &Option<Cursor<EventId>>,
    ) -> Result<CursoredResult<LocalizedNotification, EventId>, NotificationError> {
        let cursor = (*cursor).unwrap_or_default();
        let scan_index_forward = false; // newest first

        let paged_records = self
            .notification_repository
            .query_notification_records(user_id, &cursor, scan_index_forward)
            .await?;
        let last = paged_records.last().cloned();

        let notifications: Vec<LocalizedNotification> = paged_records
            .into_iter()
            .map(Notification::from)
            .map(|n| n.localized(currency, languages))
            .collect();

        let total = if notifications.is_empty() {
            0
        } else {
            self.notification_repository
                .count_notification_records(user_id, &cursor, scan_index_forward)
                .await?
        };

        Ok(CursoredResult {
            cursor: Cursor {
                size: notifications.len() as u64,
                search_after: last.map(|l| l.origin_event_id),
            },
            items: notifications,
            total: Some(total),
        })
    }
}

#[cfg(test)]
mod tests {
    mod find_notification {
        use crate::{
            dynamodb::repository::MockNotificationDynamoDbRepository,
            service::notification_service::{
                NotificationError, NotificationService, NotificationServiceImpl,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{event_id::EventId, user_id::UserId};
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_err_notification_not_found_when_no_notification_exists() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let service = NotificationServiceImpl::new(&repository);
            let user_id = UserId::new();
            let origin_event_id = EventId::new();
            let actual = service
                .find_notification(&user_id, &origin_event_id)
                .await
                .unwrap_err();

            match actual {
                NotificationError::NotificationNotFound(err_user_id, err_origin_event_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(origin_event_id, err_origin_event_id);
                }
                err => {
                    panic!("Expected 'NotificationError::NotificationNotFound' but got '{err}'")
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let service = NotificationServiceImpl::new(&repository);
            let actual = service
                .find_notification(&Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkGetItemError', got '{err}'"),
            }
        }
    }

    mod create_notification {
        use crate::{
            core::notification::{NotificationPayload, NotificationWatchlistPayload},
            dynamodb::repository::MockNotificationDynamoDbRepository,
            service::{
                command::CreateNotificationCommand,
                notification_service::{
                    NotificationError, NotificationService, NotificationServiceImpl,
                },
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::{
            language::domain::Language, product_state::domain::ProductState, user_id::UserId,
        };
        use fake::{Fake, Faker};
        use std::collections::HashMap;

        fn make_test_command() -> CreateNotificationCommand {
            CreateNotificationCommand {
                user_id: UserId::new(),
                notification_payload: NotificationPayload::Watchlist {
                    product_id: Faker.fake(),
                    shop_id: Faker.fake(),
                    shops_product_id: "test-product-123".into(),
                    shop_slug_id: Faker.fake(),
                    product_slug_id: Faker.fake(),
                    shop_name: "Test Shop".into(),
                    title: HashMap::from([(Language::En, "Test Title".into())]),
                    watchlist_payload: NotificationWatchlistPayload::StateChange {
                        old_state: ProductState::Listed,
                        new_state: ProductState::Sold,
                    },
                },
            }
        }

        #[tokio::test]
        async fn should_create_when_success() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = NotificationServiceImpl::new(&repository);
            let result = service
                .create_notification(&Faker.fake(), make_test_command())
                .await
                .unwrap();

            assert!(!result.seen);
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_put_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_record()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let service = NotificationServiceImpl::new(&repository);
            let actual = service
                .create_notification(&Faker.fake(), make_test_command())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkPutItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkPutItemError', got '{err}'"),
            }
        }
    }

    mod update_notification {
        use crate::{
            dynamodb::{
                notification_record::NotificationRecord,
                repository::MockNotificationDynamoDbRepository,
            },
            service::{
                command::UpdateNotificationCommand,
                notification_service::{
                    NotificationError, NotificationService, NotificationServiceImpl,
                },
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{event_id::EventId, user_id::UserId};
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_update_seen_when_success() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = false;
                        Ok(Some(faked))
                    })
                });
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = true;
                        Ok(Some(faked))
                    })
                });

            let service = NotificationServiceImpl::new(&repository);
            let result = service
                .update_notification(
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await
                .unwrap();

            assert!(result.seen);
        }

        #[tokio::test]
        async fn should_err_notification_not_found_when_no_notification_exists() {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let service = NotificationServiceImpl::new(&repository);
            let user_id = UserId::new();
            let origin_event_id = EventId::new();
            let actual = service
                .update_notification(
                    &user_id,
                    &origin_event_id,
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await
                .unwrap_err();

            match actual {
                NotificationError::NotificationNotFound(err_user_id, err_origin_event_id) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(origin_event_id, err_origin_event_id);
                }
                err => {
                    panic!("Expected 'NotificationError::NotificationNotFound' but got '{err}'")
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let service = NotificationServiceImpl::new(&repository);
            let actual = service
                .update_notification(
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkGetItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_update_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_get_notification_record()
                .return_once(|_, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<NotificationRecord>();
                        faked.seen = false;
                        Ok(Some(faked))
                    })
                });
            repository
                .expect_update_notification_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let service = NotificationServiceImpl::new(&repository);
            let actual = service
                .update_notification(
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateNotificationCommand { seen: Some(true) },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkUpdateItemError(_) => {}
                err => panic!("Expected 'NotificationError::SdkUpdateItemError', got '{err}'"),
            }
        }
    }

    mod view_notifications {
        use crate::{
            dynamodb::repository::MockNotificationDynamoDbRepository,
            service::notification_service::{
                NotificationError, NotificationService, NotificationServiceImpl,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{currency::domain::Currency, language::domain::Language};
        use fake::{Fake, Faker};

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_query(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_query_notification_records()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let service = NotificationServiceImpl::new(&repository);
            let actual = service
                .view_notifications(&Faker.fake(), &[Language::De], &Currency::Eur, &None)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                NotificationError::SdkQueryError(_) => {}
                err => panic!("Expected 'NotificationError::SdkQueryError', got '{err}'"),
            }
        }
    }
}
