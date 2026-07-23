use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
    sqs::{SqsEvent, SqsMessage},
};
use common::{
    batch::Batch, event_id::EventId, product_id::ProductId,
    product_lifecycle::record::ProductLifecycleRecord, resource_state::record::ResourceStateRecord,
};
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use opensearch::GetParts;
use product::{
    dynamodb::{
        product_event_record::{
            ProductEventRecord,
            lifecycle::{
                ProductLifecycleEventRecord, mk_pk as mk_product_event_pk,
                mk_sk as mk_product_lifecycle_event_sk,
            },
        },
        product_event_type_record::lifecycle::ProductLifecycleEventTypeRecord,
        product_record::ProductRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
    opensearch::{product_document::ProductDocument, repository::ProductOpenSearchRepository},
};
use product_lambda_delete_product::handler;
use product_watchlist::dynamodb::{
    record::{
        WatchlistProductRecord, mk_gsi1_pk as mk_watchlist_gsi1_pk,
        mk_gsi1_sk as mk_watchlist_gsi1_sk, mk_lsi1_sk as mk_watchlist_lsi1_sk,
        mk_pk as mk_watchlist_pk, mk_sk as mk_watchlist_sk,
    },
    repository::{WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl},
};
use search_filter::dynamodb::{
    repository::{UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl},
    user_search_filter_match_record::{
        UserSearchFilterMatchRecord, mk_gsi2_pk as mk_match_gsi2_pk,
        mk_gsi2_sk as mk_match_gsi2_sk, mk_lsi1_sk as mk_match_lsi1_sk,
        mk_lsi2_sk as mk_match_lsi2_sk, mk_pk as mk_match_pk, mk_sk as mk_match_sk,
    },
};
use std::time::{Duration, Instant};
use test_api::*;
use time::OffsetDateTime;

fn sqs_event(message: SqsMessage) -> LambdaEvent<SqsEvent> {
    let mut event = SqsEvent::default();
    event.records = vec![message];
    LambdaEvent::new(event, Context::default())
}

fn deleted_lifecycle_record(product_record: &ProductRecord) -> ProductLifecycleEventRecord {
    let mut lifecycle_record = Faker.fake::<ProductLifecycleEventRecord>();
    let event_id = EventId::new();
    lifecycle_record.pk =
        mk_product_event_pk(&product_record.shop_id, &product_record.shops_product_id);
    lifecycle_record.sk = mk_product_lifecycle_event_sk(&event_id);
    lifecycle_record.product_id = product_record.product_id;
    lifecycle_record.event_id = event_id;
    lifecycle_record.shop_id = product_record.shop_id;
    lifecycle_record.seller_id = product_record.seller_id;
    lifecycle_record.shops_product_id = product_record.shops_product_id.clone();
    lifecycle_record.event_type = ProductLifecycleEventTypeRecord::LifecycleDeleted;
    lifecycle_record.old_lifecycle = ProductLifecycleRecord::Active;
    lifecycle_record.new_lifecycle = ProductLifecycleRecord::Deleted;
    lifecycle_record.timestamp = OffsetDateTime::now_utc();
    lifecycle_record
}

fn deleted_product_message(product_record: &ProductRecord) -> SqsMessage {
    let lifecycle_record = deleted_lifecycle_record(product_record);

    let mut stream_record = StreamRecord::default();
    stream_record.new_image =
        serde_dynamo::to_item(ProductEventRecord::Lifecycle(lifecycle_record))
            .expect("product lifecycle event should serialize");

    let mut event_record = EventRecord::default();
    event_record.event_name = "INSERT".to_owned();
    event_record.change = stream_record;

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail = event_record;

    let mut message = SqsMessage::default();
    message.message_id = Some("msg-delete-product".to_owned());
    message.body = Some(serde_json::to_string(&event).expect("event should serialize"));
    message
}

fn target_watchlist_record(product_record: &ProductRecord) -> WatchlistProductRecord {
    let mut record = Faker.fake::<WatchlistProductRecord>();
    record.pk = mk_watchlist_pk(&record.user_id);
    record.sk = mk_watchlist_sk(&product_record.shop_id, &product_record.shops_product_id);
    record.lsi1_sk = mk_watchlist_lsi1_sk(&record.created);
    record.gsi1_pk = mk_watchlist_gsi1_pk(&product_record.product_id);
    record.gsi1_sk = mk_watchlist_gsi1_sk(&record.user_id);
    record.product_id = product_record.product_id;
    record.shop_id = product_record.shop_id;
    record.shops_product_id = product_record.shops_product_id.clone();
    record.state = ResourceStateRecord::Active;
    record
}

fn civilian_watchlist_record() -> WatchlistProductRecord {
    let product_id = ProductId::new();
    let mut record = Faker.fake::<WatchlistProductRecord>();
    record.pk = mk_watchlist_pk(&record.user_id);
    record.sk = mk_watchlist_sk(&record.shop_id, &record.shops_product_id);
    record.lsi1_sk = mk_watchlist_lsi1_sk(&record.created);
    record.gsi1_pk = mk_watchlist_gsi1_pk(&product_id);
    record.gsi1_sk = mk_watchlist_gsi1_sk(&record.user_id);
    record.product_id = product_id;
    record.state = ResourceStateRecord::Active;
    record
}

fn target_match_record(product_record: &ProductRecord) -> UserSearchFilterMatchRecord {
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_match_pk(&record.user_id);
    record.sk = mk_match_sk(
        &record.user_search_filter_id,
        &product_record.shop_id,
        &product_record.shops_product_id,
    );
    record.lsi1_sk = mk_match_lsi1_sk(&record.created);
    record.lsi2_sk = Some(mk_match_lsi2_sk(
        &product_record.shop_id,
        &product_record.shops_product_id,
        &record.created,
    ));
    record.gsi2_pk = Some(mk_match_gsi2_pk(&product_record.product_id));
    record.gsi2_sk = Some(mk_match_gsi2_sk(&record.user_id));
    record.product_id = product_record.product_id;
    record.shop_id = product_record.shop_id;
    record.shops_product_id = product_record.shops_product_id.clone();
    record
}

fn civilian_match_record() -> UserSearchFilterMatchRecord {
    let product_id = ProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_match_pk(&record.user_id);
    record.sk = mk_match_sk(
        &record.user_search_filter_id,
        &record.shop_id,
        &record.shops_product_id,
    );
    record.lsi1_sk = mk_match_lsi1_sk(&record.created);
    record.lsi2_sk = Some(mk_match_lsi2_sk(
        &record.shop_id,
        &record.shops_product_id,
        &record.created,
    ));
    record.gsi2_pk = Some(mk_match_gsi2_pk(&product_id));
    record.gsi2_sk = Some(mk_match_gsi2_sk(&record.user_id));
    record.product_id = product_id;
    record
}

async fn opensearch_doc_exists(index: &str, id: impl Into<String>) -> bool {
    let id = id.into();
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id))
        .send()
        .await
        .expect("get document request should finish");
    if response.status_code().as_u16() == 404 {
        return false;
    }
    response
        .error_for_status_code()
        .expect("get document response should be successful");
    true
}

async fn wait_until_cleanup_gsis_are_visible(
    watchlist_repository: &impl WatchlistProductDynamoDbRepository,
    search_filter_repository: &impl UserSearchFilterDynamoDbRepository,
    product_record: &ProductRecord,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let watchlist_visible = watchlist_repository
            .query_user_ids_watching_product(&product_record.product_id)
            .await
            .expect("watchlist gsi query should work")
            .iter()
            .any(|record| {
                record.shop_id == product_record.shop_id
                    && record.shops_product_id == product_record.shops_product_id
            });

        let match_visible = search_filter_repository
            .query_user_search_filter_match_keys_for_product_id(&product_record.product_id)
            .await
            .expect("match gsi query should work")
            .iter()
            .any(|(_, _, shop_id, shops_product_id)| {
                *shop_id == product_record.shop_id
                    && shops_product_id == &product_record.shops_product_id
            });

        if watchlist_visible && match_visible {
            return;
        }
        if Instant::now() >= deadline {
            panic!("cleanup gsi records did not become visible");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[aura_integration_test(services = [DynamoDB(), OpenSearch()])]
async fn should_delete_product_and_cleanup_user_resources_when_deleted_lifecycle_event_received() {
    let dynamodb_client = get_dynamodb_client().await;
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let search_filter_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let opensearch_repository =
        product::opensearch::repository::ProductOpenSearchRepositoryImpl::new(
            get_opensearch_client().await,
        );

    let mut product_record = Faker.fake::<ProductRecord>();
    product_record.lifecycle = ProductLifecycleRecord::Active;
    product_repository
        .put_product_records(Batch::from([product_record.clone()]))
        .await
        .expect("product record should be seeded");
    let product_event_record =
        ProductEventRecord::Lifecycle(deleted_lifecycle_record(&product_record));
    product_repository
        .put_product_event_records(Batch::from([product_event_record]))
        .await
        .expect("product event record should be seeded");
    opensearch_repository
        .create_product_documents(vec![ProductDocument::from(product_record.clone())])
        .await
        .expect("product document should be created");
    refresh_index("products").await;
    assert!(
        opensearch_doc_exists("products", product_record.product_id.to_string()).await,
        "product document should exist before delete"
    );

    let target_watchlist_record = target_watchlist_record(&product_record);
    watchlist_repository
        .put_watchlist_record(target_watchlist_record.clone())
        .await
        .expect("target watchlist record should be seeded");
    let civilian_watchlist_record = civilian_watchlist_record();
    watchlist_repository
        .put_watchlist_record(civilian_watchlist_record.clone())
        .await
        .expect("civilian watchlist record should be seeded");

    let target_match_record = target_match_record(&product_record);
    search_filter_repository
        .put_user_search_filter_match_record(target_match_record.clone())
        .await
        .expect("target match record should be seeded");
    let civilian_match_record = civilian_match_record();
    search_filter_repository
        .put_user_search_filter_match_record(civilian_match_record.clone())
        .await
        .expect("civilian match record should be seeded");

    wait_until_cleanup_gsis_are_visible(
        &watchlist_repository,
        &search_filter_repository,
        &product_record,
    )
    .await;

    let response = handler(
        &opensearch_repository,
        &watchlist_repository,
        &search_filter_repository,
        &product_repository,
        sqs_event(deleted_product_message(&product_record)),
    )
    .await
    .expect("handler should respond");

    assert!(response.batch_item_failures.is_empty());
    refresh_index("products").await;
    assert!(
        !opensearch_doc_exists("products", product_record.product_id.to_string()).await,
        "product document should be deleted"
    );
    assert!(
        watchlist_repository
            .get_watchlist_record(
                &target_watchlist_record.user_id,
                &target_watchlist_record.shop_id,
                &target_watchlist_record.shops_product_id,
            )
            .await
            .expect("target watchlist read should work")
            .is_none(),
        "target watchlist record should be deleted"
    );
    assert!(
        search_filter_repository
            .get_user_search_filter_match_record(
                &target_match_record.user_id,
                &target_match_record.user_search_filter_id,
                &target_match_record.shop_id,
                &target_match_record.shops_product_id,
            )
            .await
            .expect("target match read should work")
            .is_none(),
        "target match record should be deleted"
    );
    assert!(
        product_repository
            .get_product_record(&product_record.shop_id, &product_record.shops_product_id)
            .await
            .expect("product record read should work")
            .is_none(),
        "product record should be deleted"
    );
    assert!(
        product_repository
            .query_product_record_and_event_record_keys(
                &product_record.shop_id,
                &product_record.shops_product_id,
            )
            .await
            .expect("product event key query should work")
            .is_empty(),
        "product event records should be deleted"
    );
    assert!(
        watchlist_repository
            .get_watchlist_record(
                &civilian_watchlist_record.user_id,
                &civilian_watchlist_record.shop_id,
                &civilian_watchlist_record.shops_product_id,
            )
            .await
            .expect("civilian watchlist read should work")
            .is_some(),
        "civilian watchlist record should remain"
    );
    assert!(
        search_filter_repository
            .get_user_search_filter_match_record(
                &civilian_match_record.user_id,
                &civilian_match_record.user_search_filter_id,
                &civilian_match_record.shop_id,
                &civilian_match_record.shops_product_id,
            )
            .await
            .expect("civilian match read should work")
            .is_some(),
        "civilian match record should remain"
    );
}
