use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use product_watchlist::dynamodb::{
    record_update::WatchlistProductRecordUpdate, repository::WatchlistProductDynamoDbRepository,
};
use time::OffsetDateTime;
use tracing::{error, info};
use user::dynamodb::user_record::UserRecord;

#[tracing::instrument(skip(watchlist_repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    watchlist_repository: &impl WatchlistProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;

    for message in event.payload.records {
        let message_id = message
            .message_id
            .clone()
            .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
        if let Some(user_record) = extract_sqs_event_bridge_dynamodb_record::<UserRecord>(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
        ) {
            let watchlist_records_res = watchlist_repository
                .query_watchlist_records_all(&user_record.user_id, true)
                .await;
            match watchlist_records_res {
                Ok(watchlist_records) => {
                    for watchlist_record in watchlist_records {
                        let update = WatchlistProductRecordUpdate {
                            gsi1_pk: None,
                            gsi1_sk: None,
                            notifications: None,
                            user_record: Some(user_record.clone()),
                            updated: OffsetDateTime::now_utc(),
                        };
                        let update_res = watchlist_repository
                            .update_watchlist_record(
                                &user_record.user_id,
                                &watchlist_record.shop_id,
                                &watchlist_record.shops_product_id,
                                update,
                            )
                            .await;
                        if let Err(err) = update_res {
                            failed_message_ids.push(message_id.clone());
                            error!(
                                error = ?err,
                                "Failed updating watchlist-record for user when attempting to update denormalized user-record field inside watchlist-record.
                                 Failing entire operation for user. This may leave partially updated denormalized user-records in the users watchlist-records"
                            );
                            break;
                        }
                    }
                }
                Err(err) => {
                    failed_message_ids.push(message_id);
                    error!(
                        error = ?err,
                        "Failed querying all watchlist-records for user when attempting to update denormalized user-record field inside watchlist-record.
                         Failing entire operation for user."
                    );
                }
            }
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count - skipped_count,
        failures = failure_count,
        skipped = skipped_count,
        "Handler finished.",
    );
    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(sqs_batch_response)
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product_watchlist::dynamodb::record::WatchlistProductRecord;
    use product_watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository;
    use std::time::SystemTime;
    use user::dynamodb::user_record::UserRecord;
    use uuid::Uuid;

    fn mk_event_bridge_payload(user_record: &UserRecord) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(user_record).unwrap();
        stream_record.size_bytes = 42;

        let mut user_record_ddb = EventRecord::default();
        user_record_ddb.aws_region = "eu-central-1".to_string();
        user_record_ddb.change = stream_record;
        user_record_ddb.event_id = Uuid::new_v4().to_string();
        user_record_ddb.event_name = "MODIFY".to_string();

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "foo".to_string();
        event.source = "bar".to_string();
        event.detail = user_record_ddb;

        serde_json::to_string(&event).unwrap()
    }

    fn mk_sqs_message(record: &UserRecord) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    #[trace]
    async fn should_succeed_all_when_all_query_and_update_succeed(#[case] record_count: usize) {
        let records = fake::vec![UserRecord; record_count]
            .into_iter()
            .map(|record| mk_sqs_message(&record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut repository = MockWatchlistProductDynamoDbRepository::default();
        repository
            .expect_query_watchlist_records_all()
            .returning(move |_, _| Box::pin(async move { Ok(Faker.fake()) }));
        repository
            .expect_update_watchlist_record()
            .returning(move |_, _, _, _| Box::pin(async move { Ok(Some(Faker.fake())) }));

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    #[trace]
    async fn should_partially_fail_when_query_fails(#[case] record_count: usize) {
        let records = fake::vec![UserRecord; record_count]
            .into_iter()
            .map(|record| mk_sqs_message(&record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut repository = MockWatchlistProductDynamoDbRepository::default();
        repository
            .expect_query_watchlist_records_all()
            .returning(move |_, _| {
                Box::pin(async move { Err(SdkError::construction_failure("foo")) })
            });

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert_eq!(record_count, actual.batch_item_failures.len());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    #[trace]
    async fn should_partially_fail_when_any_update_fails(#[case] record_count: usize) {
        let records = fake::vec![UserRecord; record_count]
            .into_iter()
            .map(|record| mk_sqs_message(&record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut repository = MockWatchlistProductDynamoDbRepository::default();
        repository
            .expect_query_watchlist_records_all()
            .returning(move |_, _| {
                Box::pin(async move { Ok(fake::vec![WatchlistProductRecord; 42]) })
            });
        repository
            .expect_update_watchlist_record()
            .returning(move |_, _, _, _| {
                Box::pin(async move { Err(SdkError::construction_failure("foo")) })
            });

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert_eq!(record_count, actual.batch_item_failures.len());
    }
}
