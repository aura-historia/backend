use aws_lambda_events::dynamodb::EventRecord;
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::SqsEvent;
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use lambda_runtime::LambdaEvent;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::opensearch::repository::UserSearchFilterOpenSearchRepository;
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use tracing::{error, info};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl UserSearchFilterOpenSearchRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<(), lambda_runtime::Error> {
    let mut event = event;
    let msg = event.payload.records.remove(0);
    let body = msg.body.clone();

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let record = extract_sqs_event_bridge_dynamodb_record::<UserSearchFilterRecord>(
        msg,
        &mut failed_message_ids,
        &mut skipped_count,
    );

    if skipped_count > 0 {
        return Ok(());
    }

    match record {
        Some(record) => handle_upsert(repository, record).await,
        None => {
            // Record extraction failed — this is expected for REMOVE events since
            // new_image is empty. Parse the raw body to handle deletion.
            let body = body.expect("body must be Some when extract_sqs_event_bridge_dynamodb_record returns None with skipped_count == 0");
            let event_bridge_event = serde_json::from_str::<EventBridgeEvent<EventRecord>>(&body)
                .map_err(|e| {
                let msg = "Failed deserializing EventBridgeEvent<EventRecord>.";
                error!(error = %e, payload = %body, msg);
                lambda_runtime::Error::from(msg)
            })?;
            let event_name = event_bridge_event.detail.event_name.as_str();
            match event_name {
                "REMOVE" => handle_delete(repository, &event_bridge_event).await,
                _ => {
                    let msg = "Unexpected event: record extraction failed and event is not REMOVE.";
                    error!(event_name = event_name, msg);
                    Err(msg.into())
                }
            }
        }
    }
}

async fn handle_upsert(
    repository: &impl UserSearchFilterOpenSearchRepository,
    record: UserSearchFilterRecord,
) -> Result<(), lambda_runtime::Error> {
    let filter_id = record.user_search_filter_id;
    let document: UserSearchFilterDocument = record.into();

    match repository.index_document(document).await {
        Ok(response) => {
            info!(
                userSearchFilterId = %filter_id,
                result = response.result,
                "Indexed UserSearchFilterRecord"
            );
            Ok(())
        }
        Err(err) => {
            let msg = "Failed indexing UserSearchFilterRecord";
            error!(error = %err, userSearchFilterId = %filter_id, msg);
            Err(msg.into())
        }
    }
}

async fn handle_delete(
    repository: &impl UserSearchFilterOpenSearchRepository,
    event: &EventBridgeEvent<EventRecord>,
) -> Result<(), lambda_runtime::Error> {
    let sk: &str = event
        .detail
        .change
        .keys
        .get("sk")
        .and_then(|v| {
            if let serde_dynamo::AttributeValue::S(s) = v {
                Some(s.as_str())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            let msg = "Failed extracting 'sk' from keys. Expected a String attribute 'sk' in the DynamoDB stream record keys.";
            error!(msg);
            lambda_runtime::Error::from(msg)
        })?;

    let filter_id_str = sk.strip_prefix("search_filter#").ok_or_else(|| {
        let msg =
            "Failed parsing UserSearchFilterId from sk. Expected format 'search_filter#<uuid>'.";
        error!(sk = %sk, msg);
        lambda_runtime::Error::from(msg)
    })?;

    let filter_id = UserSearchFilterId::try_from(filter_id_str).map_err(|e| {
        let msg = "Failed converting sk to UserSearchFilterId.";
        error!(error = %e, sk = %sk, msg);
        lambda_runtime::Error::from(msg)
    })?;

    match repository.delete_document(&filter_id).await {
        Ok(response) => {
            info!(
                userSearchFilterId = %filter_id,
                result = response.result,
                "Deleted UserSearchFilterDocument"
            );
            Ok(())
        }
        Err(err) => {
            let msg = "Failed deleting UserSearchFilterDocument";
            error!(error = %err, userSearchFilterId = %filter_id, msg);
            Err(msg.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::dynamodb::StreamRecord;
    use aws_lambda_events::sqs::SqsMessage;
    use fake::{Fake, Faker};
    use lambda_runtime::Context;
    use search_filter::opensearch::repository::MockUserSearchFilterOpenSearchRepository;
    use std::collections::HashMap;

    fn mk_sqs_event(body: String) -> LambdaEvent<SqsEvent> {
        let mut msg = SqsMessage::default();
        msg.message_id = Some("test-message-id".to_string());
        msg.body = Some(body);
        LambdaEvent {
            payload: {
                let mut e = SqsEvent::default();
                e.records = vec![msg];
                e
            },
            context: Context::default(),
        }
    }

    fn mk_event_bridge_body(record: &UserSearchFilterRecord, event_name: &str) -> String {
        let new_image = serde_dynamo::to_item(record.clone()).unwrap();

        let mut stream_record = StreamRecord::default();
        stream_record.new_image = new_image;

        let mut event_record = EventRecord::default();
        event_record.event_name = event_name.to_string();
        event_record.change = stream_record;

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "DynamoDBStreamRecord".to_string();
        event.source = "table_1".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }

    fn mk_delete_event_bridge_body(
        user_search_filter_id: &UserSearchFilterId,
        user_id: &common::user_id::UserId,
    ) -> String {
        let mut keys = HashMap::new();
        keys.insert(
            "pk".to_string(),
            serde_dynamo::AttributeValue::S(format!("user#{user_id}")),
        );
        keys.insert(
            "sk".to_string(),
            serde_dynamo::AttributeValue::S(format!("search_filter#{user_search_filter_id}")),
        );

        let mut stream_record = StreamRecord::default();
        stream_record.keys = keys.into();

        let mut event_record = EventRecord::default();
        event_record.event_name = "REMOVE".to_string();
        event_record.change = stream_record;

        let mut event = EventBridgeEvent::<EventRecord>::default();
        event.detail_type = "DynamoDBStreamRecord".to_string();
        event.source = "table_1".to_string();
        event.detail = event_record;

        serde_json::to_string(&event).unwrap()
    }

    #[tokio::test]
    async fn should_index_document_when_insert_event() {
        let record = Faker.fake::<UserSearchFilterRecord>();
        let body = mk_event_bridge_body(&record, "INSERT");
        let event = mk_sqs_event(body);

        let mut mock_repo = MockUserSearchFilterOpenSearchRepository::new();
        mock_repo.expect_index_document().times(1).returning(|_| {
            Box::pin(async {
                Ok(common::opensearch::index_response::IndexResponse {
                    index: "user_search_filter".to_string(),
                    id: "test".to_string(),
                    version: Some(1),
                    result: "created".to_string(),
                })
            })
        });

        let res = handler(&mock_repo, event).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn should_index_document_when_modify_event() {
        let record = Faker.fake::<UserSearchFilterRecord>();
        let body = mk_event_bridge_body(&record, "MODIFY");
        let event = mk_sqs_event(body);

        let mut mock_repo = MockUserSearchFilterOpenSearchRepository::new();
        mock_repo.expect_index_document().times(1).returning(|_| {
            Box::pin(async {
                Ok(common::opensearch::index_response::IndexResponse {
                    index: "user_search_filter".to_string(),
                    id: "test".to_string(),
                    version: Some(2),
                    result: "updated".to_string(),
                })
            })
        });

        let res = handler(&mock_repo, event).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn should_delete_document_when_remove_event() {
        let record = Faker.fake::<UserSearchFilterRecord>();
        let filter_id = record.user_search_filter_id;
        let user_id = record.user_id;
        let body = mk_delete_event_bridge_body(&filter_id, &user_id);
        let event = mk_sqs_event(body);

        let mut mock_repo = MockUserSearchFilterOpenSearchRepository::new();
        mock_repo.expect_delete_document().times(1).returning(|_| {
            Box::pin(async {
                Ok(common::opensearch::delete_response::DeleteResponse {
                    index: "user_search_filter".to_string(),
                    id: "test".to_string(),
                    version: Some(1),
                    result: "deleted".to_string(),
                })
            })
        });

        let res = handler(&mock_repo, event).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn should_skip_when_empty_body() {
        let mut msg = SqsMessage::default();
        msg.message_id = Some("test-message-id".to_string());
        let event = LambdaEvent {
            payload: {
                let mut e = SqsEvent::default();
                e.records = vec![msg];
                e
            },
            context: Context::default(),
        };

        let mock_repo = MockUserSearchFilterOpenSearchRepository::new();
        let res = handler(&mock_repo, event).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn should_fail_when_invalid_json_body() {
        let event = mk_sqs_event("not json".to_string());
        let mock_repo = MockUserSearchFilterOpenSearchRepository::new();
        let res = handler(&mock_repo, event).await;
        assert!(res.is_err());
    }
}
