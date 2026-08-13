use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::postgres::SqlxUnitOfWork;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::transaction::{Transaction, UnitOfWork};
use lambda_runtime::{Context, LambdaEvent};
use product_postgres::{
    SqlxPartnerProductAuthorizerFactory, SqlxProductEventStoreFactory, SqlxProductRepositoryFactory,
};
use product_service::use_cases::{IngestShopifyProductHandler, UpsertProductHandler};
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation, ShopifyIntegration};
use shop_core::shop_type::ShopType;
use shop_postgres::{SqlxShopDetailsReaderFactory, SqlxShopRepositoryFactory};
use shop_service::ports::{ShopRepository, ShopRepositoryFactory};
use shop_service::use_cases::GetShopHandler;
use shopify_lambda::{
    SHOPIFY_TOPIC_PRODUCTS_CREATE, SHOPIFY_TOPIC_PRODUCTS_DELETE, SHOPIFY_TOPIC_PRODUCTS_UPDATE,
    handler,
};
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

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
async fn should_ignore_shopify_event_for_non_partner_shop() {
    let shop = seed_shop(ShopPartnerStatus::Scraped).await;
    let domain = shop
        .shopify()
        .map(|value| value.domain.as_str())
        .unwrap_or("");

    let response = invoke(SHOPIFY_TOPIC_PRODUCTS_CREATE, domain, 104, 5).await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, product_count(shop.id()).await);
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
        UpsertProductHandler::new(
            unit_of_work,
            SqlxProductRepositoryFactory::new(),
            SqlxProductEventStoreFactory::new(),
            SqlxPartnerProductAuthorizerFactory::new(),
        ),
    );
    match handler(
        event(topic, shop_domain, product_id, inventory_quantity),
        &ingestion,
    )
    .await
    {
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
    let mut event = EventBridgeEvent::default();
    event.id = Some(format!("event-{product_id}-{inventory_quantity}"));
    event.detail_type = "shopifyWebhook".to_owned();
    event.source = "aws.partner/shopify.com/test".to_owned();
    event.detail = serde_json::json!({
        "payload": {
            "id": product_id,
            "title": "Shopify Cabinet",
            "body_html": "<p>Imported cabinet</p>",
            "handle": format!("cabinet-{product_id}"),
            "status": "active",
            "variants": [{"price": "42.00", "inventory_quantity": inventory_quantity}],
            "images": [{"src": "https://images.example/cabinet.jpg"}]
        },
        "metadata": {
            "X-Shopify-Topic": topic,
            "X-Shopify-Shop-Domain": shop_domain,
            "X-Shopify-Event-Id": format!("shopify-{product_id}-{inventory_quantity}")
        }
    });
    let body = serde_json::to_string(&event)
        .unwrap_or_else(|error| panic!("failed serializing Shopify EventBridge fixture: {error}"));
    let mut message = SqsMessage::default();
    message.message_id = Some(format!("message-{product_id}-{inventory_quantity}"));
    message.body = Some(body);
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = vec![message];
    LambdaEvent::new(sqs_event, Context::default())
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
