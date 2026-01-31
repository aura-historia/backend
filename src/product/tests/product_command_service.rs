use common::{price::domain::FixedFxRate, product_state::domain::ProductState};
use product::core::product::Product;
use product::dynamodb::product_event_record::ProductDomainEventRecordSerdeField;
use product::dynamodb::{
    product_event_record::ProductDomainEventRecord,
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::{
    product_command::UpsertProductCommand,
    upsert_service::{UpsertProductsService, UpsertProductsServiceImpl},
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_write_all_products_to_dynamodb_as_created_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UpsertProductsServiceImpl::new(&repository, &FixedFxRate());

    let commands = fake::vec![UpsertProductCommand; 543];
    let output = service.upsert(commands.clone()).await;
    assert!(output.unprocessed.is_empty());
    assert_eq!(0, output.skipped);

    let all_event_records_created = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .iter()
        .all(|record| {
            record
                .get(ProductDomainEventRecordSerdeField::EventType.as_str())
                .unwrap()
                .as_s()
                .unwrap()
                == "CREATED"
        });
    assert!(all_event_records_created);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_no_product_events_when_all_exist_and_no_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UpsertProductsServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![UpsertProductCommand; 400];
    for cmd in cmds.clone() {
        let event_record: ProductDomainEventRecord = Product::create(
            cmd.shop_id,
            cmd.shops_product_id,
            cmd.shop_name,
            cmd.shop_type,
            cmd.native_title,
            cmd.native_description,
            cmd.native_price,
            Default::default(),
            None,
            Default::default(),
            None,
            Default::default(),
            cmd.state,
            cmd.url,
            cmd.images,
            cmd.auction_start,
            cmd.auction_end,
        )
        .try_into()
        .unwrap();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    let output = service.upsert(cmds).await;
    assert!(output.unprocessed.is_empty());
    assert_eq!(400, output.skipped);
    let actual_records = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap_or_default()
        .len();
    assert_eq!(400, actual_records); // just the existing materialized ones
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_product_updates_when_all_exist_and_actual_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UpsertProductsServiceImpl::new(&repository, &FixedFxRate());

    let mut cmds = fake::vec![UpsertProductCommand; 400];
    for cmd in cmds.clone() {
        let event_record: ProductDomainEventRecord = Product::create(
            cmd.shop_id,
            cmd.shops_product_id,
            cmd.shop_name,
            cmd.shop_type,
            cmd.native_title,
            cmd.native_description,
            cmd.native_price,
            Default::default(),
            None,
            Default::default(),
            None,
            Default::default(),
            cmd.state,
            cmd.url,
            cmd.images,
            cmd.auction_start,
            cmd.auction_end,
        )
        .try_into()
        .unwrap();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    // actual change to previously (from cmd derived) materialized product-record
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

    let all_event_records_update_state_available = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .iter()
        .filter_map(|record| record.get(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .all(|val| val.as_s().unwrap() == "STATE_AVAILABLE");
    assert!(all_event_records_update_state_available);
}
