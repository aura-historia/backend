use common::{
    has_key::HasKey, price::domain::FixedFxRate, product_id::ProductKey,
    product_state::domain::ProductState,
};
use fake::{Fake, Faker};
use product::core::product::Product;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecordSerdeField;
use product::dynamodb::{
    product_event_record::domain::ProductDomainEventRecord,
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::{
    command_service::{CommandProductService, CommandProductServiceImpl},
    product_command::{CreateProductCommand, UpdateProductCommand},
};
use std::collections::HashMap;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_write_all_products_to_dynamodb_as_created_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let commands = fake::vec![CreateProductCommand; 543];
    let failures = service.create(commands.clone()).await;
    assert!(failures.is_empty());

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
                == "DOMAIN_CREATED"
        });
    assert!(all_event_records_created);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_not_create_duplicate_products_when_already_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![CreateProductCommand; 400];
    // First create
    let failures = service.create(cmds.clone()).await;
    assert!(failures.is_empty());

    // Second create of the same products should not fail but should skip existing
    let failures = service.create(cmds).await;
    assert!(failures.is_empty());

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
    // Only the original create events should exist
    assert_eq!(400, actual_records);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_no_product_update_events_when_all_exist_and_no_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![CreateProductCommand; 400];
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
        .into();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    let update_cmds: HashMap<ProductKey, UpdateProductCommand> = cmds
        .into_iter()
        .map(|cmd| {
            (
                ProductKey {
                    shop_id: cmd.shop_id,
                    shops_product_id: cmd.shops_product_id,
                },
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: cmd.state,
                },
            )
        })
        .collect();

    let failures = service.update(update_cmds).await;
    assert!(failures.is_empty());
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
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![CreateProductCommand; 400];
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
        .into();
        let product_record: ProductRecord = event_record.try_into().unwrap();
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty())
    }

    let update_cmds: HashMap<ProductKey, UpdateProductCommand> = cmds
        .into_iter()
        .map(|cmd| {
            (
                ProductKey {
                    shop_id: cmd.shop_id,
                    shops_product_id: cmd.shops_product_id,
                },
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: ProductState::Available,
                },
            )
        })
        .collect();

    let failures = service.update(update_cmds).await;
    assert!(failures.is_empty());

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
        .all(|val| val.as_s().unwrap() == "DOMAIN_STATE_CHANGED");
    assert!(all_event_records_update_state_available);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_failures_when_updating_non_existent_products() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..5)
        .map(|_| {
            let cmd = Faker.fake::<CreateProductCommand>();
            (
                cmd.key(),
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: cmd.state,
                },
            )
        })
        .collect();
    let expected_keys: Vec<ProductKey> = cmds.keys().cloned().collect();
    let failures = service.update(cmds).await;

    let mut actual_keys: Vec<ProductKey> = failures.keys().cloned().collect();
    let mut expected_sorted = expected_keys;
    expected_sorted.sort();
    actual_keys.sort();

    assert_eq!(expected_sorted, actual_keys);
}
