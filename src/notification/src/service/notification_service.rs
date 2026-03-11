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
    batch::Batch,
    currency::domain::Currency,
    event_id::EventId,
    language::domain::Language,
    pagination::cursor::{Cursor, CursoredResult},
    user_id::UserId,
};
use std::collections::{HashMap, HashSet};
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

    async fn send_externally(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError>;
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
            notification_type: None,
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
        if cmds.is_empty() {
            return CreateNotificationsResult {
                unprocessed: vec![],
                processed: vec![],
            };
        }

        let now = OffsetDateTime::now_utc();

        // Build (cmd, notification) pairs and keep a record clone for batching.
        let pairs: Vec<(CreateNotificationCommand, Notification)> = cmds
            .into_iter()
            .map(|cmd| {
                let notification = Notification {
                    user_id: cmd.user_id,
                    origin_event_id: *origin_event_id,
                    notification_id: NotificationId::new(),
                    notification_type: None,
                    notification_payload: cmd.notification_payload.clone(),
                    seen: false,
                    created: now,
                    updated: now,
                };
                (cmd, notification)
            })
            .collect();

        // Index by user_id so we can look up cmd/notification after batch responses.
        let mut cmd_map: HashMap<UserId, (CreateNotificationCommand, Notification)> = pairs
            .into_iter()
            .map(|(cmd, notif)| (notif.user_id, (cmd, notif)))
            .collect();

        let records: Vec<NotificationRecord> = cmd_map
            .values()
            .map(|(_, notif)| NotificationRecord::from(notif.clone()))
            .collect();

        let mut processed = Vec::new();
        let mut unprocessed = Vec::new();

        let batches = Batch::<NotificationRecord, 25>::chunked_from(records.into_iter());
        for batch in batches {
            let user_ids_in_batch: Vec<UserId> = batch.iter().map(|r| r.user_id).collect();

            match self
                .notification_repository
                .put_notification_records(batch)
                .await
            {
                Ok(output) => {
                    let failed_user_ids: HashSet<UserId> = output
                        .unprocessed_items
                        .unwrap_or_default()
                        .into_iter()
                        .flat_map(|(_, write_reqs)| write_reqs)
                        .filter_map(|req| req.put_request)
                        .filter_map(|put| {
                            match serde_dynamo::from_item::<_, NotificationRecord>(put.item) {
                                Ok(record) => Some(record.user_id),
                                Err(err) => {
                                    tracing::error!(
                                        error = ?err,
                                        r#type = std::any::type_name::<NotificationRecord>(),
                                        "Failed parsing unprocessed item from BatchWriteItem output"
                                    );
                                    None
                                }
                            }
                        })
                        .collect();

                    for user_id in user_ids_in_batch {
                        if let Some((cmd, notif)) = cmd_map.remove(&user_id) {
                            if failed_user_ids.contains(&user_id) {
                                unprocessed.push((
                                    cmd,
                                    NotificationError::SdkPutItemError(
                                        SdkError::construction_failure(
                                            "Item returned as unprocessed in BatchWriteItem",
                                        ),
                                    ),
                                ));
                            } else {
                                processed.push(notif);
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(
                        error = ?err,
                        "Failed writing NotificationRecord batch due to SdkError."
                    );
                    for user_id in user_ids_in_batch {
                        if let Some((cmd, _)) = cmd_map.remove(&user_id) {
                            unprocessed.push((
                                cmd,
                                NotificationError::SdkPutItemError(SdkError::construction_failure(
                                    "BatchWriteItem operation failed",
                                )),
                            ));
                        }
                    }
                }
            }
        }

        CreateNotificationsResult {
            unprocessed,
            processed,
        }
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
                notification_type: None,
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

    async fn send_externally(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Notification, NotificationError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    mod create_notifications {
        use crate::{
            core::notification::{NotificationPayload, NotificationWatchlistPayload},
            dynamodb::repository::MockNotificationDynamoDbRepository,
            service::{
                command::CreateNotificationCommand,
                notification_service::{
                    CreateNotificationsResult, NotificationService, NotificationServiceImpl,
                },
            },
        };
        use aws_sdk_dynamodb::{
            operation::batch_write_item::BatchWriteItemOutput,
            types::{PutRequest, WriteRequest},
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
        async fn should_return_empty_when_no_commands() {
            // No mock expectations set — any call to the repository would cause a panic.
            let repository = MockNotificationDynamoDbRepository::default();
            let service = NotificationServiceImpl::new(&repository);

            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service.create_notifications(&Faker.fake(), vec![]).await;

            assert!(processed.is_empty());
            assert!(unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_create_notifications_when_all_succeed() {
            let cmds: Vec<CreateNotificationCommand> =
                (0..3).map(|_| make_test_command()).collect();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_records()
                .return_once(|_| Box::pin(async { Ok(BatchWriteItemOutput::builder().build()) }));

            let service = NotificationServiceImpl::new(&repository);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service.create_notifications(&Faker.fake(), cmds).await;

            assert_eq!(processed.len(), 3);
            assert!(unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_mark_unprocessed_when_batch_write_returns_unprocessed_items() {
            use crate::dynamodb::notification_record::NotificationRecord;
            use common::batch::Batch;

            let cmd_to_fail = make_test_command();
            let failing_user_id = cmd_to_fail.user_id;
            let cmd_to_succeed = make_test_command();

            let cmds = vec![cmd_to_fail, cmd_to_succeed];

            // Capture the batch the service sends so we can pull out the exact serialised item
            // for the failing user and return it as an unprocessed entry.  This ensures the
            // DynamoDB attribute map we hand back is byte-for-byte what serde_dynamo produced.
            let mut repository = MockNotificationDynamoDbRepository::default();
            repository.expect_put_notification_records().return_once(
                move |batch: Batch<NotificationRecord, 25>| {
                    // Find the record belonging to the user we want to fail.
                    let failing_item = batch
                        .iter()
                        .find(|r| r.user_id == failing_user_id)
                        .and_then(|r| serde_dynamo::to_item(r).ok())
                        .expect("failing record must be in the batch");

                    let unprocessed_write_req = WriteRequest::builder()
                        .put_request(
                            PutRequest::builder()
                                .set_item(Some(failing_item))
                                .build()
                                .unwrap(),
                        )
                        .build();
                    let output = BatchWriteItemOutput::builder()
                        .unprocessed_items("irrelevant_table", vec![unprocessed_write_req])
                        .build();

                    Box::pin(async move { Ok(output) })
                },
            );

            let service = NotificationServiceImpl::new(&repository);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service.create_notifications(&Faker.fake(), cmds).await;

            assert_eq!(processed.len(), 1);
            assert_eq!(unprocessed.len(), 1);
            assert_eq!(unprocessed[0].0.user_id, failing_user_id);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error_batch_write() {
            let cmds: Vec<CreateNotificationCommand> =
                (0..3).map(|_| make_test_command()).collect();

            let mut repository = MockNotificationDynamoDbRepository::default();
            repository
                .expect_put_notification_records()
                .return_once(|_| {
                    Box::pin(async {
                        Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Simulated BatchWriteItem failure",
                        ))
                    })
                });

            let service = NotificationServiceImpl::new(&repository);
            let CreateNotificationsResult {
                processed,
                unprocessed,
            } = service.create_notifications(&Faker.fake(), cmds).await;

            assert!(processed.is_empty());
            assert_eq!(unprocessed.len(), 3);
        }
    }

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
