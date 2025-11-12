use common::{product_state::domain::ProductState, price::domain::FixedFxRate};
use product::core::item::Item;
use product::dynamodb::{
    item_event_record::ProductEventRecord,
    item_event_type_record::ProductEventTypeRecord,
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::{
    item_command::UpsertItemCommand,
    upsert_service::{UpsertItemsService, UpsertItemsServiceImpl},
};
use test_api::*;

const INGEST_ITEM_QUEUE: Sqs = Sqs {
    name: "ingest-item-queue",
};

#[localstack_test(services = [DynamoDB(), INGEST_ITEM_QUEUE])]
async fn should_push_all_items_to_queue_as_created_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let sqs_client = get_sqs_client().await;
    let q_url = INGEST_ITEM_QUEUE.queue_url();
    let service = UpsertItemsServiceImpl::new(&repository, sqs_client, &q_url, &FixedFxRate());

    let output = service.upsert(fake::vec![UpsertItemCommand; 543]).await;
    assert!(output.unprocessed.is_empty());
    assert_eq!(0, output.skipped);

    loop {
        let received = sqs_client
            .receive_message()
            .queue_url(&q_url)
            .max_number_of_messages(10)
            .visibility_timeout(600)
            .send()
            .await
            .unwrap();

        match received.messages.unwrap_or_default().as_slice() {
            &[] => break,
            msgs => {
                for msg in msgs {
                    let event_record: ProductEventRecord =
                        serde_json::from_str(msg.body().unwrap()).unwrap();
                    assert_eq!(ProductEventTypeRecord::Created, event_record.event_type);
                }
            }
        }
    }
}

#[localstack_test(services = [DynamoDB(), INGEST_ITEM_QUEUE])]
async fn should_push_no_items_to_queue_when_all_exist_and_no_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let sqs_client = get_sqs_client().await;
    let q_url = INGEST_ITEM_QUEUE.queue_url();
    let service = UpsertItemsServiceImpl::new(&repository, sqs_client, &q_url, &FixedFxRate());

    let cmds = fake::vec![UpsertItemCommand; 400];
    for cmd in cmds.clone() {
        let event_record: ProductEventRecord = Item::create(
            cmd.shop_id,
            cmd.shops_product_id,
            cmd.shop_name,
            cmd.native_title,
            Default::default(),
            cmd.native_description,
            Default::default(),
            cmd.native_price,
            Default::default(),
            cmd.state,
            cmd.url,
            cmd.images,
        )
        .try_into()
        .unwrap();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_item_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    let output = service.upsert(cmds).await;
    assert!(output.unprocessed.is_empty());
    assert_eq!(400, output.skipped);

    let received = sqs_client
        .receive_message()
        .queue_url(&q_url)
        .max_number_of_messages(10)
        .visibility_timeout(600)
        .send()
        .await
        .unwrap();

    assert!(received.messages().is_empty());
}

#[localstack_test(services = [DynamoDB(), INGEST_ITEM_QUEUE])]
async fn should_push_items_to_queue_when_all_exist_and_actual_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let sqs_client = get_sqs_client().await;
    let q_url = INGEST_ITEM_QUEUE.queue_url();
    let service = UpsertItemsServiceImpl::new(&repository, sqs_client, &q_url, &FixedFxRate());

    let mut cmds = fake::vec![UpsertItemCommand; 400];
    for cmd in cmds.clone() {
        let event_record: ProductEventRecord = Item::create(
            cmd.shop_id,
            cmd.shops_product_id,
            cmd.shop_name,
            cmd.native_title,
            Default::default(),
            cmd.native_description,
            Default::default(),
            cmd.native_price,
            Default::default(),
            cmd.state,
            cmd.url,
            cmd.images,
        )
        .try_into()
        .unwrap();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_item_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    // actual change to previously (from cmd derived) materialiezd item-record
    // does not imply a change for all because some may already have the "new" state
    let mut expected_skipped = 0;
    for cmd in &mut cmds {
        if cmd.state == ProductState::Available {
            expected_skipped += 1;
        } else {
            cmd.state = ProductState::Available
        }
    }

    let output = service.upsert(cmds).await;
    assert!(output.unprocessed.is_empty());
    assert_eq!(expected_skipped, output.skipped);

    loop {
        let received = sqs_client
            .receive_message()
            .queue_url(&q_url)
            .max_number_of_messages(10)
            .visibility_timeout(600)
            .send()
            .await
            .unwrap();

        match received.messages.unwrap_or_default().as_slice() {
            &[] => break,
            msgs => {
                for msg in msgs {
                    let event_record: ProductEventRecord =
                        serde_json::from_str(msg.body().unwrap()).unwrap();
                    assert_eq!(ProductEventTypeRecord::StateAvailable, event_record.event_type);
                }
            }
        }
    }
}
