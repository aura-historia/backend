use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::fx_rate_id::FxRateId;
use common::postgres::SqlxUnitOfWork;
use common::transaction::{Transaction, UnitOfWork};
use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryFactory,
};
use lambda_runtime::{Context, LambdaEvent};
use localization::Language;
use money::Currency;
use product_postgres::{
    SqlxPartnerProductAuthorizerFactory, SqlxProductEventStoreFactory, SqlxProductRepositoryFactory,
};
use product_service::use_cases::{IngestShopifyProductHandler, UpsertProductHandler};
use shop_core::domain::Domain;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation, ShopifyIntegration};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_type::ShopType;
use shop_postgres::{SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory};
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::GetShopHandler;
use shopify_lambda::{
    SHOPIFY_TOPIC_PRODUCTS_CREATE, SHOPIFY_TOPIC_PRODUCTS_DELETE, SHOPIFY_TOPIC_PRODUCTS_UPDATE,
    handler,
};
use std::collections::HashSet;
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_create_product_and_event_in_postgres_for_shopify_create() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;

    let response = invoke(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        shop.shopify()
            .map(|value| value.domain.as_str())
            .unwrap_or(""),
        100,
        5,
    )
    .await;

    assert!(response.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 100).await;
    assert_eq!("AVAILABLE", product.state);
    assert_eq!(4_200, product.price_amount);
    assert_eq!("USD", product.price_currency);
    assert_eq!(1, product_event_count(product.product_id).await);
    assert_eq!(
        "PRODUCT_CREATED",
        latest_event_type(product.product_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_create() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");

    let first = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 101, 5).await;
    let second = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 101, 5).await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 101).await;
    assert_eq!(1, product_event_count(product.product_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_append_state_event_in_postgres_for_shopify_update() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    let created = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 102, 5).await;
    assert!(created.batch_item_failures.is_empty());

    seed_canonical_fx_snapshot().await;
    let updated = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 102, 0).await;

    assert!(updated.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 102).await;
    assert_eq!("SOLD", product.state);
    assert_eq!(4_200, product.price_amount);
    assert_eq!("USD", product.price_currency);
    assert_eq!(2, product_event_count(product.product_id).await);
    assert_eq!(
        "PRODUCT_STATE_CHANGED",
        latest_event_type(product.product_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_mark_product_removed_and_append_event_for_shopify_delete() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    let created = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 103, 5).await;
    assert!(created.batch_item_failures.is_empty());

    let deleted = invoke(SHOPIFY_TOPIC_PRODUCTS_DELETE, domain, 103, 5).await;

    assert!(deleted.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 103).await;
    assert_eq!("REMOVED", product.state);
    assert_eq!(2, product_event_count(product.product_id).await);
    assert_eq!(
        "PRODUCT_STATE_CHANGED",
        latest_event_type(product.product_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_ignore_shopify_event_for_unpartnered_shop() {
    let shop = seed_shop(ShopPartnerStatus::Scraped).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");

    let response = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 104, 5).await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count(shop.id()).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_ignore_shopify_event_for_missing_shop() {
    let response = invoke(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        "missing-shop.example",
        105,
        5,
    )
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count_for_shops_product_id(105).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retry_malformed_sqs_body_without_persisting_product() {
    let response = invoke_event(sqs_event("malformed-sqs", Some("not-json".to_owned()))).await;

    assert_eq!(vec!["malformed-sqs"], failure_ids(response));
    assert_eq!(0, product_count_for_shops_product_id(106).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retry_malformed_eventbridge_detail_without_persisting_product() {
    let response = invoke_event(event_with_detail(
        "malformed-detail",
        "event-malformed-detail",
        serde_json::json!({
            "payload": shopify_payload(107, 5),
            "metadata": {"X-Shopify-Topic": SHOPIFY_TOPIC_PRODUCTS_CREATE}
        }),
    ))
    .await;

    assert_eq!(vec!["malformed-detail"], failure_ids(response));
    assert_eq!(0, product_count_for_shops_product_id(107).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_unsupported_topic_without_persisting_product() {
    let response = invoke_event(event_with_detail(
        "unsupported-topic",
        "event-unsupported-topic",
        shopify_detail("orders/create", "unsupported-topic.example", 108, 5),
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count_for_shops_product_id(108).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_invalid_shop_domain_without_persisting_product() {
    let response = invoke_event(event_with_detail(
        "invalid-domain",
        "event-invalid-domain",
        shopify_detail(SHOPIFY_TOPIC_PRODUCTS_CREATE, "not a domain", 109, 5),
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count_for_shops_product_id(109).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_missing_title_without_persisting_product() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    let mut payload = shopify_payload(110, 5);
    payload["title"] = serde_json::Value::Null;

    let response = invoke_event(event_with_detail(
        "missing-title",
        "event-missing-title",
        serde_json::json!({
            "payload": payload,
            "metadata": {
                "X-Shopify-Topic": SHOPIFY_TOPIC_PRODUCTS_CREATE,
                "X-Shopify-Shop-Domain": domain,
                "X-Shopify-Event-Id": "shopify-missing-title"
            }
        }),
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count(shop.id()).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_invalid_price_without_persisting_product() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    let mut payload = shopify_payload(111, 5);
    payload["variants"][0]["price"] = serde_json::json!("not-a-price");

    let response = invoke_event(event_with_detail(
        "invalid-price",
        "event-invalid-price",
        serde_json::json!({
            "payload": payload,
            "metadata": {
                "X-Shopify-Topic": SHOPIFY_TOPIC_PRODUCTS_CREATE,
                "X-Shopify-Shop-Domain": domain,
                "X-Shopify-Event-Id": "shopify-invalid-price"
            }
        }),
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count(shop.id()).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_update() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    assert!(
        invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 112, 5)
            .await
            .batch_item_failures
            .is_empty()
    );

    seed_canonical_fx_snapshot().await;
    let first = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 112, 0).await;
    let second = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 112, 0).await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 112).await;
    assert_eq!("SOLD", product.state);
    assert_eq!(2, product_event_count(product.product_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_delete() {
    let shop = seed_shop(ShopPartnerStatus::Partnered).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");
    assert!(
        invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 113, 5)
            .await
            .batch_item_failures
            .is_empty()
    );

    let first = invoke(SHOPIFY_TOPIC_PRODUCTS_DELETE, domain, 113, 5).await;
    let second = invoke(SHOPIFY_TOPIC_PRODUCTS_DELETE, domain, 113, 5).await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    let product = product_row(shop.id(), 113).await;
    assert_eq!("REMOVED", product.state);
    assert_eq!(2, product_event_count(product.product_id).await);
}

async fn seed_canonical_fx_snapshot() {
    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Eur {
                    FX_RATE_SCALE
                } else {
                    1_250_000
                },
            )
        }),
    )
    .unwrap_or_else(|error| panic!("valid canonical FX test fixture: {error}"));
    let unit_of_work = SqlxUnitOfWork::new(get_postgres_client().await);
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("start canonical FX test fixture transaction: {error}"));
    let outcome = SqlxFxRateSnapshotRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&snapshot, &format!("shopify-fx-{}", FxRateId::new()))
        .await
        .unwrap_or_else(|error| panic!("insert canonical FX test fixture: {error}"));
    assert!(matches!(outcome, FxRateSnapshotInsertOutcome::Inserted(_)));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit canonical FX test fixture: {error}"));
}

async fn invoke(
    topic: &str,
    shop_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
) -> aws_lambda_events::sqs::SqsBatchResponse {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let ingestion = IngestShopifyProductHandler::new(
        GetShopHandler::new(unit_of_work.clone(), SqlxShopDetailsReaderFactory::new()),
        UpsertProductHandler::new_with_fx_rates(
            unit_of_work,
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        ),
    );
    invoke_with_ingestion(
        event(topic, shop_domain, product_id, inventory_quantity),
        &ingestion,
    )
    .await
}

async fn invoke_event(event: LambdaEvent<SqsEvent>) -> aws_lambda_events::sqs::SqsBatchResponse {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let ingestion = IngestShopifyProductHandler::new(
        GetShopHandler::new(unit_of_work.clone(), SqlxShopDetailsReaderFactory::new()),
        UpsertProductHandler::new_with_fx_rates(
            unit_of_work,
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        ),
    );
    invoke_with_ingestion(event, &ingestion).await
}

async fn invoke_with_ingestion(
    event: LambdaEvent<SqsEvent>,
    ingestion: &(dyn product_service::use_cases::IngestShopifyProductUseCase + Send + Sync),
) -> aws_lambda_events::sqs::SqsBatchResponse {
    match handler(event, ingestion).await {
        Ok(response) => response,
        Err(error) => panic!("Shopify handler failed: {error}"),
    }
}

async fn seed_shop(partner_status: ShopPartnerStatus) -> Shop {
    let domain = Domain::try_from(format!("shopify-{}.example", ShopId::new()).as_str())
        .unwrap_or_else(|error| panic!("invalid Shopify domain: {error}"));
    let mut shop = Shop::create(NewShop {
        id: ShopId::new(),
        name: ShopName::from(format!("Shopify integration {domain}").as_str()),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([domain.clone()]),
        shopify: Some(ShopifyIntegration {
            domain,
            currency: Some(Currency::Usd),
            language: Some(Language::De),
        }),
        woocommerce: None,
        presentation: ShopPresentation::default(),
        address: None,
        contact: ShopContact::default(),
        partner_status,
        affiliate_configuration: None,
    });
    match shop.publish() {
        Ok(_) => {}
        Err(error) => panic!("failed publishing shop fixture: {error}"),
    }

    let unit_of_work = SqlxUnitOfWork::new(get_postgres_client().await);
    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed starting shop fixture transaction: {error}"),
    };
    match SqlxShopRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&shop)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed inserting shop fixture: {error}"),
    }
    match tx.commit().await {
        Ok(_) => shop,
        Err(error) => panic!("failed committing shop fixture: {error}"),
    }
}

fn event(
    topic: &str,
    shop_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
) -> LambdaEvent<SqsEvent> {
    event_with_detail(
        &format!("message-{product_id}-{inventory_quantity}"),
        &format!("event-{product_id}-{inventory_quantity}"),
        shopify_detail(topic, shop_domain, product_id, inventory_quantity),
    )
}

fn shopify_detail(
    topic: &str,
    shop_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
) -> serde_json::Value {
    serde_json::json!({
        "payload": shopify_payload(product_id, inventory_quantity),
        "metadata": {
            "X-Shopify-Topic": topic,
            "X-Shopify-Shop-Domain": shop_domain,
            "X-Shopify-Event-Id": format!("shopify-{product_id}-{inventory_quantity}")
        }
    })
}

fn shopify_payload(product_id: u64, inventory_quantity: i64) -> serde_json::Value {
    serde_json::json!({
        "id": product_id,
        "title": "Shopify Cabinet",
        "body_html": "<p>Imported cabinet</p>",
        "handle": format!("cabinet-{product_id}"),
        "status": "active",
        "variants": [{"price": "42.00", "inventory_quantity": inventory_quantity}],
        "images": [{"src": "https://images.example/cabinet.jpg"}]
    })
}

fn event_with_detail(
    message_id: &str,
    event_id: &str,
    detail: serde_json::Value,
) -> LambdaEvent<SqsEvent> {
    let mut event = EventBridgeEvent::default();
    event.id = Some(event_id.to_owned());
    event.detail_type = "shopifyWebhook".to_owned();
    event.source = "aws.partner/shopify.com/test".to_owned();
    event.detail = detail;
    let body = serde_json::to_string(&event)
        .unwrap_or_else(|error| panic!("failed serializing Shopify EventBridge fixture: {error}"));
    sqs_event(message_id, Some(body))
}

fn sqs_event(message_id: &str, body: Option<String>) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some(message_id.to_owned());
    message.body = body;
    let mut event = SqsEvent::default();
    event.records = vec![message];
    LambdaEvent::new(event, Context::default())
}

struct ProductRow {
    product_id: uuid::Uuid,
    state: String,
    price_amount: i64,
    price_currency: String,
}

async fn product_row(shop_id: ShopId, product_id: u64) -> ProductRow {
    match sqlx::query_as::<_, (uuid::Uuid, String, i64, String)>(
        "SELECT product_id, state, price_amount, price_currency FROM products WHERE shop_id = $1 AND shops_product_id = $2",
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(product_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    {
        Ok((product_id, state, price_amount, price_currency)) => ProductRow {
            product_id,
            state,
            price_amount,
            price_currency,
        },
        Err(error) => panic!("failed loading Shopify product row: {error}"),
    }
}

async fn product_event_count(product_id: uuid::Uuid) -> i64 {
    match sqlx::query_scalar("SELECT COUNT(*) FROM product_events WHERE product_id = $1")
        .bind(product_id)
        .fetch_one(&get_postgres_client().await)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting product events: {error}"),
    }
}

async fn latest_event_type(product_id: uuid::Uuid) -> String {
    match sqlx::query_scalar(
        "SELECT event_type FROM product_events WHERE product_id = $1 ORDER BY event_time DESC LIMIT 1",
    )
    .bind(product_id)
    .fetch_one(&get_postgres_client().await)
    .await
    {
        Ok(event_type) => event_type,
        Err(error) => panic!("failed loading latest product event: {error}"),
    }
}

async fn product_count(shop_id: ShopId) -> i64 {
    match sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE shop_id = $1")
        .bind(uuid::Uuid::from(shop_id))
        .fetch_one(&get_postgres_client().await)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting Shopify products: {error}"),
    }
}

async fn product_count_for_shops_product_id(product_id: u64) -> i64 {
    match sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE shops_product_id = $1")
        .bind(product_id.to_string())
        .fetch_one(&get_postgres_client().await)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting Shopify products by external ID: {error}"),
    }
}

fn failure_ids(response: aws_lambda_events::sqs::SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}
