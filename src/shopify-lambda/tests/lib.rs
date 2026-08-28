use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};

use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::{Domain, ListingSourceId};
use listing_source_postgres::SqlxListingSourceReaders;
use listing_source_service::use_cases::queries::get_shopify_source::GetSystemShopifySourceHandler;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingEventStoreFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    IngestShopifyProductListingHandler, UpsertProductListingHandler, WithdrawProductListingHandler,
};
use shopify_lambda::{
    SHOPIFY_TOPIC_PRODUCTS_CREATE, SHOPIFY_TOPIC_PRODUCTS_DELETE, SHOPIFY_TOPIC_PRODUCTS_UPDATE,
    ShopifyProductListingProcessor, ShopifyProductListingProcessorUseCase, handler,
};

use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_create_product_listing_and_event_in_postgres_for_shopify_create() {
    let source = seed_source().await;

    let response = invoke(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        source.domain.as_str(),
        100,
        5,
    )
    .await;

    assert!(response.batch_item_failures.is_empty());
    let listing = listing_row(source.id, 100).await;
    assert_eq!(Some("IN_STOCK"), listing.availability.as_deref());
    assert_eq!("ACTIVE", listing.lifecycle);
    assert_eq!(4_200, listing.price_amount);
    assert_eq!("USD", listing.price_currency);
    assert_eq!(1, listing_event_count(listing.product_listing_id).await);
    assert_eq!(
        "PRODUCT_LISTING_CREATED",
        current_event_type(listing.product_listing_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_create() {
    let source = seed_source().await;
    let domain = source.domain.as_str();

    let first = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 101, 5).await;
    let second = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 101, 5).await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    let listing = listing_row(source.id, 101).await;
    assert_eq!(1, listing_event_count(listing.product_listing_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_append_availability_event_in_postgres_for_shopify_update() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let created = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 102, 5).await;
    assert!(created.batch_item_failures.is_empty());

    let updated = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 102, 0).await;

    assert!(updated.batch_item_failures.is_empty());
    let listing = listing_row(source.id, 102).await;
    assert_eq!(Some("OUT_OF_STOCK"), listing.availability.as_deref());
    assert_eq!("ACTIVE", listing.lifecycle);
    assert_eq!(4_200, listing.price_amount);
    assert_eq!("USD", listing.price_currency);
    assert_eq!(2, listing_event_count(listing.product_listing_id).await);
    assert_eq!(
        "PRODUCT_LISTING_AVAILABILITY_CHANGED",
        current_event_type(listing.product_listing_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_withdraw_product_listing_and_append_event_for_shopify_delete() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let created = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 103, 5).await;
    assert!(created.batch_item_failures.is_empty());

    let deleted = invoke(SHOPIFY_TOPIC_PRODUCTS_DELETE, domain, 103, 5).await;

    assert!(deleted.batch_item_failures.is_empty());
    let listing = listing_row(source.id, 103).await;
    assert_eq!(None, listing.availability);
    assert_eq!("WITHDRAWN", listing.lifecycle);
    assert_eq!(2, listing_event_count(listing.product_listing_id).await);
    assert_eq!(
        "PRODUCT_LISTING_WITHDRAWN",
        current_event_type(listing.product_listing_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_ignore_shopify_event_for_missing_listing_source() {
    let response = invoke(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        "missing-source.example",
        105,
        5,
    )
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, listing_count_for_source_listing_id(105).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retry_malformed_sqs_body_without_persisting_product() {
    let response = invoke_event(sqs_event("malformed-sqs", Some("not-json".to_owned()))).await;

    assert_eq!(vec!["malformed-sqs"], failure_ids(response));
    assert_eq!(0, listing_count_for_source_listing_id(106).await);
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
    assert_eq!(0, listing_count_for_source_listing_id(107).await);
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
    assert_eq!(0, listing_count_for_source_listing_id(108).await);
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
    assert_eq!(0, listing_count_for_source_listing_id(109).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_missing_title_without_persisting_product() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
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
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_invalid_price_without_persisting_product() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
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
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_update() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    assert!(
        invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 112, 5)
            .await
            .batch_item_failures
            .is_empty()
    );

    let first = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 112, 0).await;
    let second = invoke(SHOPIFY_TOPIC_PRODUCTS_UPDATE, domain, 112, 0).await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    let listing = listing_row(source.id, 112).await;
    assert_eq!(Some("OUT_OF_STOCK"), listing.availability.as_deref());
    assert_eq!("ACTIVE", listing.lifecycle);
    assert_eq!(2, listing_event_count(listing.product_listing_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_append_duplicate_event_for_redelivered_shopify_delete() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
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
    let listing = listing_row(source.id, 113).await;
    assert_eq!(None, listing.availability);
    assert_eq!("WITHDRAWN", listing.lifecycle);
    assert_eq!(2, listing_event_count(listing.product_listing_id).await);
}

async fn invoke(
    topic: &str,
    source_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
) -> aws_lambda_events::sqs::SqsBatchResponse {
    let pool = get_postgres_client().await;
    let processor = shopify_product_listing_processor(pool);
    invoke_with_processor(
        event(topic, source_domain, product_id, inventory_quantity),
        &processor,
    )
    .await
}

async fn invoke_event(event: LambdaEvent<SqsEvent>) -> aws_lambda_events::sqs::SqsBatchResponse {
    let processor = shopify_product_listing_processor(get_postgres_client().await);
    invoke_with_processor(event, &processor).await
}

fn shopify_product_listing_processor(
    pool: sqlx::PgPool,
) -> impl ShopifyProductListingProcessorUseCase {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let sources = SqlxListingSourceReaders::new(pool);
    ShopifyProductListingProcessor::new(
        sources.clone(),
        IngestShopifyProductListingHandler::new(
            GetSystemShopifySourceHandler::new(sources),
            UpsertProductListingHandler::new(
                unit_of_work.clone(),
                SqlxProductListingRepositoryFactory::new(),
                SqlxProductListingEventStoreFactory::new(),
                SqlxPartnerProductListingAuthorizerFactory::new(),
            ),
        ),
        WithdrawProductListingHandler::new(
            unit_of_work,
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventStoreFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        ),
    )
}

async fn invoke_with_processor(
    event: LambdaEvent<SqsEvent>,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
) -> aws_lambda_events::sqs::SqsBatchResponse {
    match handler(event, processor).await {
        Ok(response) => response,
        Err(error) => panic!("Shopify handler failed: {error}"),
    }
}

struct ShopifySourceFixture {
    id: ListingSourceId,
    domain: Domain,
}

async fn seed_source() -> ShopifySourceFixture {
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::new_v4();
    let domain = Domain::try_from(format!("shopify-{listing_source_id}.example").as_str())
        .unwrap_or_else(|error| panic!("invalid Shopify domain: {error}"));
    let pool = get_postgres_client().await;

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(operator_party_id)
        .bind(format!("operator-{operator_party_id}"))
        .bind("Shopify operator")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting source operator: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(format!("shopify-source-{listing_source_id}"))
        .bind("Shopify source")
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting listing source: {error}"));
    sqlx::query("INSERT INTO listing_source_acquisition_methods (listing_source_id, acquisition_method) VALUES ($1, 'SHOPIFY')")
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify acquisition method: {error}"));
    sqlx::query("INSERT INTO listing_source_shopify_configurations (listing_source_id, domain, currency, language) VALUES ($1, $2, 'USD', 'de')")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain.as_str())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify source configuration: {error}"));

    ShopifySourceFixture {
        id: listing_source_id,
        domain,
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
        "variants": [{"price": "42.00", "inventory_quantity": inventory_quantity, "inventory_management": "shopify"}],
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

struct ProductListingRow {
    product_listing_id: uuid::Uuid,
    availability: Option<String>,
    lifecycle: String,
    price_amount: i64,
    price_currency: String,
}

async fn listing_row(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> ProductListingRow {
    match sqlx::query_as::<_, (uuid::Uuid, Option<String>, String, i64, String)>(
        "SELECT product_listing_id, availability, lifecycle, price_amount, price_currency FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    {
        Ok((product_listing_id, availability, lifecycle, price_amount, price_currency)) => ProductListingRow {
            product_listing_id,
            availability,
            lifecycle,
            price_amount,
            price_currency,
        },
        Err(error) => panic!("failed loading Shopify product listing row: {error}"),
    }
}

async fn listing_event_count(product_listing_id: uuid::Uuid) -> i64 {
    match sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = $1",
    )
    .bind(product_listing_id)
    .fetch_one(&get_postgres_client().await)
    .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting product listing events: {error}"),
    }
}

async fn current_event_type(product_listing_id: uuid::Uuid) -> String {
    match sqlx::query_scalar(
        "SELECT event_type FROM product_listing_events WHERE product_listing_id = $1 AND event_id = (SELECT event_id FROM product_listings WHERE product_listing_id = $1)",
    )
    .bind(product_listing_id)
    .fetch_one(&get_postgres_client().await)
    .await
    {
        Ok(event_type) => event_type,
        Err(error) => panic!("failed loading current product listing event: {error}"),
    }
}

async fn listing_count(listing_source_id: ListingSourceId) -> i64 {
    match sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1")
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_one(&get_postgres_client().await)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting Shopify product listings: {error}"),
    }
}

async fn listing_count_for_source_listing_id(source_listing_id: u64) -> i64 {
    match sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE source_listing_id = $1")
        .bind(source_listing_id.to_string())
        .fetch_one(&get_postgres_client().await)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed counting Shopify product listings by external ID: {error}"),
    }
}

fn failure_ids(response: aws_lambda_events::sqs::SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}
