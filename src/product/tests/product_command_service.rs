use common::{
    currency::domain::Currency,
    has_key::HasKey,
    price::domain::{FixedFxRate, FxRate, MonetaryAmount, Price},
    product_id::ProductKey,
    product_state::domain::ProductState,
    shop_name::ShopName,
    utm::append_utm_params,
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
    product_command::{CreateProductCommand, UpdateProductCommand, UpsertProductCommand},
};
use shop::core::shop::Shop;
use shop::core::shop_type::ShopType;
use shop::service::get_service::MockGetShopService;
use std::collections::HashMap;
use test_api::*;

fn default_shop_service() -> MockGetShopService {
    let mut service = MockGetShopService::default();
    service.expect_find_shop().returning(|shop_id| {
        let mut shop: Shop = Faker.fake();
        shop.shop_id = *shop_id;
        shop.name = ShopName::from("Test Shop");
        shop.shop_type = ShopType::AuctionHouse;
        Box::pin(async move { Ok(shop) })
    });
    service
}

async fn command_product_service<'a>(
    repository: &'a (dyn ProductDynamoDbRepository + Sync),
) -> CommandProductServiceImpl<'a> {
    let get_shop_service = Box::leak(Box::new(default_shop_service()));
    CommandProductServiceImpl::new(repository, get_shop_service)
}

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

fn exchange_all(price: Option<Price>) -> HashMap<Currency, MonetaryAmount> {
    price
        .and_then(|p| {
            FixedFxRate()
                .exchange_all(p.currency, p.monetary_amount)
                .ok()
        })
        .unwrap_or_default()
}

/// Creates a `ProductRecord` from a `CreateProductCommand` with correctly
/// computed `other_price` and `other_price_estimate_min/max` via `FixedFxRate`,
/// so subsequent updates with the same field values do not spuriously generate
/// price-change or estimate-price-change events.
fn make_product_record(cmd: &CreateProductCommand) -> ProductRecord {
    let event_record: ProductDomainEventRecord = Product::create(
        cmd.shop_id,
        cmd.shop_id,
        cmd.shops_product_id.clone(),
        ShopName::from("Test Shop"),
        ShopName::from("Test Shop"),
        ShopType::AuctionHouse,
        cmd.structured_address.clone(),
        cmd.geo_address,
        cmd.native_title.clone(),
        cmd.native_description.clone(),
        cmd.native_price,
        exchange_all(cmd.native_price),
        cmd.native_price_estimate_min,
        exchange_all(cmd.native_price_estimate_min),
        cmd.native_price_estimate_max,
        exchange_all(cmd.native_price_estimate_max),
        cmd.state,
        cmd.url.clone(),
        append_utm_params(cmd.url.clone()),
        cmd.images.clone(),
        cmd.auction_start,
        cmd.auction_end,
    )
    .into();
    event_record.try_into().unwrap()
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_write_all_products_to_dynamodb_as_created_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_not_create_duplicate_products_when_already_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_write_no_product_update_events_when_all_exist_and_no_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

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
                    native_price_estimate_min: None,
                    native_price_estimate_max: None,
                    url: None,
                    images: None,
                    auction_start: None,
                    auction_end: None,
                    ..Default::default()
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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_write_product_updates_when_all_exist_and_actual_changes() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

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
                    native_price_estimate_min: None,
                    native_price_estimate_max: None,
                    url: None,
                    images: None,
                    auction_start: None,
                    auction_end: None,
                    ..Default::default()
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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_return_failures_when_updating_non_existent_products() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..5)
        .map(|_| {
            let cmd = Faker.fake::<CreateProductCommand>();
            (
                cmd.key(),
                UpdateProductCommand {
                    native_price: cmd.native_price,
                    state: Some(cmd.state),
                    native_price_estimate_min: None,
                    native_price_estimate_max: None,
                    url: None,
                    images: None,
                    auction_start: None,
                    auction_end: None,
                    ..Default::default()
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

#[aura_integration_test(services = [DynamoDB()])]
async fn should_create_new_products_via_upsert_when_none_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    let cmds: Vec<product::service::product_command::UpsertProductCommand> =
        fake::vec![product::service::product_command::UpsertProductCommand; 5];
    let failures = service.upsert(cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    let event_count = items
        .iter()
        .filter(|r| {
            r.contains_key(
                product::dynamodb::product_event_record::domain::ProductDomainEventRecordSerdeField::EventType.as_str(),
            )
        })
        .count();
    assert_eq!(5, event_count);

    let all_created = items
        .iter()
        .filter_map(|record| {
            record.get(
                product::dynamodb::product_event_record::domain::ProductDomainEventRecordSerdeField::EventType.as_str(),
            )
        })
        .all(|val| val.as_s().unwrap() == "DOMAIN_CREATED");
    assert!(all_created);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_update_existing_products_via_upsert_when_all_exist() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    let create_cmds = fake::vec![CreateProductCommand; 5];
    for cmd in &create_cmds {
        let product_record = make_product_record(cmd);
        let unprocessed = repository
            .put_product_records([product_record].into())
            .await
            .unwrap()
            .unprocessed_items
            .unwrap_or_default();
        assert!(unprocessed.is_empty());
    }

    // Upsert with state change to trigger events
    let upsert_cmds: Vec<product::service::product_command::UpsertProductCommand> = create_cmds
        .iter()
        .map(
            |cmd| product::service::product_command::UpsertProductCommand {
                shop_id: cmd.shop_id,
                shops_product_id: cmd.shops_product_id.clone(),
                seller_name_raw: cmd.seller_name_raw.clone(),
                structured_address: cmd.structured_address.clone(),
                geo_address: cmd.geo_address,
                native_title: Some(cmd.native_title.clone()),
                native_description: cmd.native_description.clone(),
                native_price: cmd.native_price,
                native_price_estimate_min: cmd.native_price_estimate_min,
                native_price_estimate_max: cmd.native_price_estimate_max,
                state: Some(ProductState::Available),
                url: Some(cmd.url.clone()),
                images: cmd.images.clone(),
                auction_start: cmd.auction_start,
                auction_end: cmd.auction_end,
            },
        )
        .collect();

    let failures = service.upsert(upsert_cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    // Every event record written must be a state-change event (no price-change events
    // because we used the same native_price).
    let all_event_records_are_state_changed = items
        .iter()
        .filter_map(|record| {
            record.get(
                product::dynamodb::product_event_record::domain::ProductDomainEventRecordSerdeField::EventType.as_str(),
            )
        })
        .all(|val| val.as_s().unwrap() == "DOMAIN_STATE_CHANGED");
    assert!(all_event_records_are_state_changed);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_merge_duplicate_upsert_commands_for_same_product() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    let create_cmd: CreateProductCommand = Faker.fake();
    let existing_record = make_product_record(&create_cmd);
    let unprocessed = repository
        .put_product_records([existing_record].into())
        .await
        .unwrap()
        .unprocessed_items
        .unwrap_or_default();
    assert!(unprocessed.is_empty());

    let price_only = UpsertProductCommand {
        shop_id: create_cmd.shop_id,
        shops_product_id: create_cmd.shops_product_id.clone(),
        seller_name_raw: None,
        structured_address: None,
        geo_address: None,
        native_title: None,
        native_description: None,
        native_price: Some(Price::new(7800u64.into(), Currency::Eur)),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        state: None,
        url: None,
        images: create_cmd.images.clone(),
        auction_start: None,
        auction_end: None,
    };
    let state_only = UpsertProductCommand {
        shop_id: create_cmd.shop_id,
        shops_product_id: create_cmd.shops_product_id.clone(),
        seller_name_raw: None,
        structured_address: None,
        geo_address: None,
        native_title: None,
        native_description: None,
        native_price: None,
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        state: Some(ProductState::Reserved),
        url: None,
        images: create_cmd.images.clone(),
        auction_start: None,
        auction_end: None,
    };

    let failures = service.upsert(vec![price_only, state_only]).await;
    assert!(failures.is_empty());

    let product = repository
        .get_product_record(&create_cmd.shop_id, &create_cmd.shops_product_id)
        .await
        .unwrap()
        .expect("product should exist after merged upsert");

    assert_eq!(
        Some(7800),
        product.price_native.as_ref().map(|price| price.amount)
    );
    assert_eq!(
        product::dynamodb::product_state_record::ProductStateRecord::Reserved,
        product.state
    );
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_create_and_update_mixed_products_via_upsert() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    // Create 3 existing products
    let existing_cmds = fake::vec![CreateProductCommand; 3];
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

    // Build upsert commands: 3 existing (update) + 2 new (create)
    let new_upsert_cmds: Vec<product::service::product_command::UpsertProductCommand> =
        fake::vec![product::service::product_command::UpsertProductCommand; 2];
    let existing_upsert_cmds: Vec<product::service::product_command::UpsertProductCommand> =
        existing_cmds
            .iter()
            .map(
                |cmd| product::service::product_command::UpsertProductCommand {
                    shop_id: cmd.shop_id,
                    shops_product_id: cmd.shops_product_id.clone(),
                    seller_name_raw: cmd.seller_name_raw.clone(),
                    structured_address: cmd.structured_address.clone(),
                    geo_address: cmd.geo_address,
                    native_title: Some(cmd.native_title.clone()),
                    native_description: cmd.native_description.clone(),
                    native_price: cmd.native_price,
                    native_price_estimate_min: None,
                    native_price_estimate_max: None,
                    state: Some(ProductState::Available),
                    url: Some(cmd.url.clone()),
                    images: cmd.images.clone(),
                    auction_start: cmd.auction_start,
                    auction_end: cmd.auction_end,
                },
            )
            .collect();

    let mut all_upsert_cmds = existing_upsert_cmds;
    all_upsert_cmds.extend(new_upsert_cmds);

    let failures = service.upsert(all_upsert_cmds).await;
    assert!(failures.is_empty());

    let items = scan_all_items().await;

    // Should have creation events for the 2 new products
    let created_count = items
        .iter()
        .filter_map(|record| {
            record.get(
                product::dynamodb::product_event_record::domain::ProductDomainEventRecordSerdeField::EventType.as_str(),
            )
        })
        .filter(|val| val.as_s().unwrap() == "DOMAIN_CREATED")
        .count();
    assert_eq!(2, created_count);
}

// ---------------------------------------------------------------------------
// Upsert retry / concurrent-create behavior (integration test)
//
// Verifies that sequential upserts for the same product converge correctly:
//   1. First upsert: product does not exist → create path → product record written.
//   2. Second upsert with a state change → update path → product state updated.
//
// This mirrors the retry scenario: when a ConditionalCheckFailed occurs on the
// first create, SQS retries the command. On retry the product exists and the
// second invocation lands on the update path, applying the state change.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [DynamoDB()])]
async fn should_converge_state_when_upsert_is_retried_after_concurrent_create() {
    let repository = ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = command_product_service(&repository).await;

    let cmd: product::service::product_command::UpsertProductCommand = Faker.fake();

    // First upsert: product does not exist → resolve to create.
    let failures_first = service.upsert(vec![cmd.clone()]).await;
    assert!(
        failures_first.is_empty(),
        "first upsert (create path) should succeed: {:?}",
        failures_first
    );

    // Product record must be present.
    let stored = repository
        .get_product_record(&cmd.shop_id, &cmd.shops_product_id)
        .await
        .unwrap();
    assert!(
        stored.is_some(),
        "product record must exist after first upsert"
    );

    // Second upsert with an explicit state change → resolve to update.
    let upsert_with_state = product::service::product_command::UpsertProductCommand {
        state: Some(common::product_state::domain::ProductState::Sold),
        ..cmd.clone()
    };
    let failures_second = service.upsert(vec![upsert_with_state]).await;
    assert!(
        failures_second.is_empty(),
        "second upsert (update path) should succeed: {:?}",
        failures_second
    );

    // Product record should reflect the state change.
    let updated = repository
        .get_product_record(&cmd.shop_id, &cmd.shops_product_id)
        .await
        .unwrap()
        .expect("product record must still exist after second upsert");
    assert_eq!(
        product::dynamodb::product_state_record::ProductStateRecord::Sold,
        updated.state,
        "product state must be Sold after second upsert"
    );
}
