use common::{
    has_key::HasKey,
    price::domain::{FixedFxRate, FxRate},
    product_id::ProductKey,
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

/// Scans all items across all pages from `table_1`.
///
/// DynamoDB returns at most 1 MB per `scan` call.  Tests that write hundreds
/// of large records must paginate to collect every item; a single-page scan
/// silently truncates the result set and causes flaky count assertions.
async fn scan_all_items() -> Vec<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> {
    let client = get_dynamodb_client().await;
    let mut all_items = Vec::new();
    let mut exclusive_start_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> =
        None;

    loop {
        let mut req = client.scan().table_name("table_1");
        if let Some(start_key) = exclusive_start_key {
            req = req.set_exclusive_start_key(Some(start_key));
        }
        let output = req.send().await.unwrap();
        all_items.extend(output.items.unwrap_or_default());
        exclusive_start_key = output.last_evaluated_key;
        if exclusive_start_key.is_none() {
            break;
        }
    }

    all_items
}

/// Creates a `ProductRecord` from a `CreateProductCommand` with correctly
/// computed `other_price` via `FixedFxRate`, so subsequent updates with the
/// same `native_price` do not spuriously generate price-change events.
fn make_product_record(cmd: &CreateProductCommand) -> ProductRecord {
    let other_price = cmd
        .native_price
        .and_then(|p| {
            FixedFxRate()
                .exchange_all(p.currency, p.monetary_amount)
                .ok()
        })
        .unwrap_or_default();
    let event_record: ProductDomainEventRecord = Product::create(
        cmd.shop_id,
        cmd.shops_product_id.clone(),
        cmd.shop_name.clone(),
        cmd.shop_type,
        cmd.native_title.clone(),
        cmd.native_description.clone(),
        cmd.native_price,
        other_price,
        None,
        Default::default(),
        None,
        Default::default(),
        cmd.state,
        cmd.url.clone(),
        cmd.images.clone(),
        cmd.auction_start,
        cmd.auction_end,
    )
    .into();
    event_record.try_into().unwrap()
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_all_products_to_dynamodb_as_created_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let commands = fake::vec![CreateProductCommand; 543];
    let failures = service.create(commands.clone()).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    let event_count = items
        .iter()
        .filter(|r| r.contains_key(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .count();
    assert_eq!(543, event_count);

    let all_created = items
        .iter()
        .filter_map(|record| record.get(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .all(|val| val.as_s().unwrap() == "DOMAIN_CREATED");
    assert!(all_created);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_not_create_duplicate_products_when_already_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    // Simulate already-materialized products by writing ProductRecord items directly.
    // The create service checks for ProductRecord existence (not event records) to
    // determine whether a product already exists.
    let existing_cmds = fake::vec![CreateProductCommand; 5];
    for cmd in &existing_cmds {
        let product_record = make_product_record(cmd);
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty());
    }

    // New products that do not yet have a ProductRecord in the table.
    let new_cmds = fake::vec![CreateProductCommand; 3];

    let mut all_cmds = existing_cmds.clone();
    all_cmds.extend(new_cmds.clone());

    let failures = service.create(all_cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    // Only 3 event records should have been written (for the 3 new products).
    let event_count = items
        .iter()
        .filter(|r| r.contains_key(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .count();
    assert_eq!(3, event_count);

    // All 3 event records must be creation events.
    let all_created = items
        .iter()
        .filter_map(|r| r.get(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .all(|val| val.as_s().unwrap() == "DOMAIN_CREATED");
    assert!(all_created);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_no_product_update_events_when_all_exist_and_no_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![CreateProductCommand; 400];
    for cmd in &cmds {
        let product_record = make_product_record(cmd);
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty());
    }

    // Update with the exact same price and state — no changes should be detected.
    let update_cmds: HashMap<ProductKey, UpdateProductCommand> = cmds
        .iter()
        .map(|cmd| {
            (
                ProductKey {
                    shop_id: cmd.shop_id,
                    shops_product_id: cmd.shops_product_id.clone(),
                },
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: Some(cmd.state),
                },
            )
        })
        .collect();

    let failures = service.update(update_cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    // No event records should have been written — only the original 400 product records.
    let event_count = items
        .iter()
        .filter(|r| r.contains_key(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .count();
    assert_eq!(0, event_count);
    assert_eq!(400, items.len());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_product_updates_when_all_exist_and_actual_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = CommandProductServiceImpl::new(&repository, &FixedFxRate());

    let cmds = fake::vec![CreateProductCommand; 400];
    for cmd in &cmds {
        let product_record = make_product_record(cmd);
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty());
    }

    // Update state to Available — products not already in Available will generate events.
    // Keep native_price unchanged so no price-change events are emitted.
    let update_cmds: HashMap<ProductKey, UpdateProductCommand> = cmds
        .iter()
        .map(|cmd| {
            (
                ProductKey {
                    shop_id: cmd.shop_id,
                    shops_product_id: cmd.shops_product_id.clone(),
                },
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: Some(ProductState::Available),
                },
            )
        })
        .collect();

    let failures = service.update(update_cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    // Every event record written must be a state-change event (no price-change events).
    let all_event_records_are_state_changed = items
        .iter()
        .filter_map(|record| record.get(ProductDomainEventRecordSerdeField::EventType.as_str()))
        .all(|val| val.as_s().unwrap() == "DOMAIN_STATE_CHANGED");
    assert!(all_event_records_are_state_changed);
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
                    state: Some(cmd.state),
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
