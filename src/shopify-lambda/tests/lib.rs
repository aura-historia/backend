use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::batch::Batch;
use common::domain::Domain;
use common::price::domain::FixedFxRate;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use serde_json::{Value, json};
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use shopify_lambda::{
    SHOPIFY_TOPIC_PRODUCTS_CREATE, SHOPIFY_TOPIC_PRODUCTS_DELETE, SHOPIFY_TOPIC_PRODUCTS_UPDATE,
    handler,
};
use test_api::*;
use time::OffsetDateTime;

// The Shopify product id used across all payload tests.
const SHOPIFY_PRODUCT_ID: u64 = 10_231_453_024_539;
const SHOPIFY_DOMAIN: &str = "aura-historia-partner-connect-dev-store.myshopify.com";

/// Builds a real-world Shopify EventBridge detail for the given topic.
fn real_shopify_eb_detail(topic: &str) -> Value {
    json!({
        "payload": {
            "admin_graphql_api_id": "gid://shopify/Product/10231453024539",
            "body_html": "<p>Hallo Test Beschreibung!</p>",
            "created_at": "2026-05-11T11:02:26-04:00",
            "handle": "thomas-testprodukt",
            "id": SHOPIFY_PRODUCT_ID,
            "product_type": "",
            "published_at": "2026-05-11T11:02:29-04:00",
            "template_suffix": "",
            "title": "Thomas Testprodukt",
            "updated_at": "2026-05-11T11:06:59-04:00",
            "vendor": "aura-historia-partner-connect-dev-store",
            "status": "active",
            "published_scope": "global",
            "tags": "",
            "variants": [
                {
                    "admin_graphql_api_id": "gid://shopify/ProductVariant/52195041706267",
                    "barcode": "",
                    "compare_at_price": null,
                    "created_at": "2026-05-11T11:02:28-04:00",
                    "id": 52_195_041_706_267_u64,
                    "inventory_policy": "deny",
                    "position": 1,
                    "price": "420.00",
                    "product_id": SHOPIFY_PRODUCT_ID,
                    "sku": null,
                    "taxable": false,
                    "title": "Default Title",
                    "updated_at": "2026-05-11T11:06:59-04:00",
                    "option1": "Default Title",
                    "option2": null,
                    "option3": null,
                    "image_id": null,
                    "inventory_item_id": 54_261_482_422_555_u64,
                    "inventory_quantity": 0,
                    "old_inventory_quantity": 0
                }
            ],
            "images": [
                {
                    "id": 51_278_835_679_515_u64,
                    "product_id": SHOPIFY_PRODUCT_ID,
                    "position": 1,
                    "created_at": "2026-05-11T11:01:04-04:00",
                    "updated_at": "2026-05-11T11:02:28-04:00",
                    "alt": null,
                    "width": 480,
                    "height": 270,
                    "src": "https://cdn.shopify.com/s/files/1/1023/7100/0603/files/Wesley-can-Gaalen-rcm480x0u.jpg?v=1778511665",
                    "variant_ids": [],
                    "admin_graphql_api_id": "gid://shopify/ProductImage/51278835679515"
                }
            ]
        },
        "metadata": {
            "Content-Type": "application/json",
            "X-Shopify-Topic": topic,
            "X-Shopify-Shop-Domain": SHOPIFY_DOMAIN,
            "X-Shopify-Product-Id": SHOPIFY_PRODUCT_ID.to_string(),
            "X-Shopify-Hmac-SHA256": "PONGKZvuaN7j92Fdw/6EKtpsx3EUOf9JZ4NrUvEi5MI=",
            "X-Shopify-Webhook-Id": "f267a21b-b283-58e6-923a-0f3be44ce67c",
            "X-Shopify-API-Version": "2026-04",
            "X-Shopify-Event-Id": "a2533945-511e-45bb-a0fa-1dba0ad9ecb1",
            "X-Shopify-Triggered-At": "2026-05-11T15:06:59.110521905Z"
        }
    })
}

/// Builds a Shopify EventBridge detail with inventory > 0 (state=Available).
fn real_shopify_eb_detail_available(topic: &str) -> Value {
    let mut detail = real_shopify_eb_detail(topic);
    detail["payload"]["variants"][0]["inventory_quantity"] = json!(5);
    detail
}

/// Wraps an EventBridge detail into an SQS LambdaEvent with a single message.
fn real_shopify_sqs_event(topic: &str) -> LambdaEvent<SqsEvent> {
    real_shopify_sqs_event_from_detail(real_shopify_eb_detail(topic))
}

fn real_shopify_sqs_event_available(topic: &str) -> LambdaEvent<SqsEvent> {
    real_shopify_sqs_event_from_detail(real_shopify_eb_detail_available(topic))
}

fn real_shopify_sqs_event_from_detail(detail: Value) -> LambdaEvent<SqsEvent> {
    let mut eb_event = EventBridgeEvent::<Value>::default();
    eb_event.id = Some("33305a23-4886-4909-8a6a-42ef59c41fe2".to_owned());
    eb_event.detail_type = "shopifyWebhook".to_owned();
    eb_event.source = "aws.partner/shopify.com/359227195393/aura-historia-backend-dev".to_owned();
    eb_event.detail = detail;

    let body = serde_json::to_string(&eb_event).unwrap();
    let mut msg = SqsMessage::default();
    msg.message_id = Some("test-msg-id".to_owned());
    msg.body = Some(body);
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = vec![msg];
    LambdaEvent::new(sqs_event, Context::default())
}

/// Seeds a Shopify partner shop record in DynamoDB and returns the shop record.
async fn seed_shopify_partner_shop(repository: &ShopDynamoDbRepositoryImpl<'_>) -> ShopRecord {
    let shopify_domain = Domain::try_from(SHOPIFY_DOMAIN).unwrap();
    let user_id = common::user_id::UserId::new();
    let shop_id = common::shop_id::ShopId::new();
    let slug = common::slug_id::SlugId::raw("shopify-test-shop");
    let now = OffsetDateTime::now_utc();

    let record = ShopRecord {
        pk: shop::dynamodb::shop_record::mk_pk(&shop_id),
        sk: shop::dynamodb::shop_record::mk_sk().to_owned(),
        shop_id,
        shop_slug_id: slug.clone(),
        name: ShopName::from("Shopify Test Shop"),
        shop_type: ShopTypeRecord::Marketplace,
        domains: Default::default(),
        shopify_domain: Some(shopify_domain.clone()),
        shopify_currency: Some(common::currency::record::CurrencyRecord::Usd),
        shopify_language: Some(common::language::record::LanguageRecord::De),
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        view_url: None,
        image: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        partner_api_key_short: None,
        partner_api_key_long_hash: None,
        partner_user_id: Some(user_id),
        gsi1_pk: Some(shop::dynamodb::shop_record::mk_gsi1_pk(&user_id)),
        gsi1_sk: Some(shop::dynamodb::shop_record::mk_gsi1_sk(&shop_id)),
        gsi2_pk: Some(shop::dynamodb::shop_record::mk_gsi2_pk(&slug)),
        gsi2_sk: Some(shop::dynamodb::shop_record::mk_gsi2_sk().to_owned()),
        gsi3_pk: Some(shop::dynamodb::shop_record::mk_gsi3_pk(&shopify_domain)),
        gsi3_sk: Some(shop::dynamodb::shop_record::mk_gsi3_sk().to_owned()),
        affiliate_configuration: None,
        created: now,
        updated: now,
    };

    repository.put_shop_record(record.clone()).await.unwrap();
    record
}

/// Seeds a materialized ProductRecord for the Shopify product so that subsequent
/// `upsert()` calls treat it as an update rather than a create.
async fn seed_product_record(
    product_repo: &ProductDynamoDbRepositoryImpl<'_>,
    shop_id: common::shop_id::ShopId,
    state: ProductStateRecord,
) -> ProductRecord {
    let shops_product_id = ShopsProductId::from(SHOPIFY_PRODUCT_ID.to_string());
    let mut record: ProductRecord = Faker.fake();
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id.clone();
    record.pk = product::dynamodb::product_record::mk_pk(&shop_id, &shops_product_id);
    record.sk = product::dynamodb::product_record::mk_sk().to_owned();
    record.state = state;
    product_repo
        .put_product_records(Batch::from([record.clone()]))
        .await
        .unwrap();
    record
}

async fn get_repositories() -> (
    ShopDynamoDbRepositoryImpl<'static>,
    ProductDynamoDbRepositoryImpl<'static>,
) {
    let client = get_dynamodb_client().await;
    let table = "table_1";
    (
        ShopDynamoDbRepositoryImpl::new(client, table),
        ProductDynamoDbRepositoryImpl::new(client, table),
    )
}

/// Returns all domain event records for the given shop and product.
async fn query_events(
    product_repo: &ProductDynamoDbRepositoryImpl<'_>,
    shop_id: common::shop_id::ShopId,
) -> Vec<ProductDomainEventRecord> {
    let shops_product_id = ShopsProductId::from(SHOPIFY_PRODUCT_ID.to_string());
    product_repo
        .query_product_domain_event_records(&shop_id, &shops_product_id)
        .await
        .unwrap()
}

// ─── Tests ───────────────────────────────────────────────────────────────────
//
// The `upsert()` service method transact-writes both a `ProductEventRecord` and
// a `ProductRecord` (materialized product state) atomically to DynamoDB.
// Integration tests assert on both the domain event records and the materialized
// product record that is written inline by the command service.

#[localstack_test(services = [DynamoDB()])]
async fn should_write_domain_created_event_when_shopify_create_event_with_real_payload() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repo,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    let result = handler(
        real_shopify_sqs_event(SHOPIFY_TOPIC_PRODUCTS_CREATE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "expected no batch failures"
    );

    let events = query_events(&product_repo, shop_id).await;
    assert!(
        !events.is_empty(),
        "expected at least one domain event record after products/create"
    );
    assert!(
        events
            .iter()
            .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainCreated),
        "expected a DOMAIN_CREATED event record; got: {:?}",
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
    assert_eq!(events[0].shop_id, shop_id);
    assert_eq!(
        events[0].shops_product_id,
        ShopsProductId::from(SHOPIFY_PRODUCT_ID.to_string())
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_domain_created_event_when_shopify_update_event_without_existing_product() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repo,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    // Without a materialized ProductRecord, upsert treats the update as a new
    // product and writes a DOMAIN_CREATED event.
    let result = handler(
        real_shopify_sqs_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "expected no batch failures"
    );

    let events = query_events(&product_repo, shop_id).await;
    assert!(
        !events.is_empty(),
        "expected at least one domain event record after products/update"
    );
    assert!(
        events
            .iter()
            .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainCreated),
        "expected a DOMAIN_CREATED event record; got: {:?}",
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
    assert_eq!(events[0].shop_id, shop_id);
}

/// This test seeds a materialized ProductRecord (state=Sold) and then fires a
/// products/update event with inventory_quantity > 0 (state=Available). Because
/// the product already exists, the service generates a DOMAIN_STATE_CHANGED event
/// rather than a DOMAIN_CREATED one.
#[localstack_test(services = [DynamoDB()])]
async fn should_write_state_changed_event_when_shopify_update_event_with_existing_product() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    // Seed a materialized record with state=Sold so that a subsequent update
    // from Available triggers a DOMAIN_STATE_CHANGED event.
    seed_product_record(&product_repo, shop_id, ProductStateRecord::Sold).await;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repo,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    // inventory_quantity = 5 → state=Available, which differs from Sold.
    let result = handler(
        real_shopify_sqs_event_available(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "expected no batch failures"
    );

    let events = query_events(&product_repo, shop_id).await;
    assert!(
        !events.is_empty(),
        "expected at least one domain event record after update"
    );
    let state_changed = events
        .iter()
        .find(|e| e.event_type == ProductDomainEventTypeRecord::DomainStateChanged);
    assert!(
        state_changed.is_some(),
        "expected a DOMAIN_STATE_CHANGED event; got: {:?}",
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
    assert_eq!(
        state_changed.unwrap().new_state,
        Some(ProductStateRecord::Available),
        "expected new_state=Available"
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_write_removed_state_event_when_shopify_delete_event_with_real_payload() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repo,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    // products/delete maps to ProductState::Removed. Without a materialized
    // ProductRecord the delete is processed as a new product creation with
    // state=Removed, producing a DOMAIN_CREATED event whose new_state is Removed.
    let result = handler(
        real_shopify_sqs_event(SHOPIFY_TOPIC_PRODUCTS_DELETE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "expected no batch failures"
    );

    let events = query_events(&product_repo, shop_id).await;
    assert!(
        !events.is_empty(),
        "expected at least one domain event record after products/delete"
    );
    let removed_event = events
        .iter()
        .find(|e| e.new_state == Some(ProductStateRecord::Removed));
    assert!(
        removed_event.is_some(),
        "expected a domain event with new_state=Removed; got: {:?}",
        events
            .iter()
            .map(|e| (&e.event_type, &e.new_state))
            .collect::<Vec<_>>()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_report_partial_failure_when_one_message_cannot_be_processed() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let seller_service = MockSellerService::default();
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let product_service = CommandProductServiceImpl::new(
        &product_repo,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail creating CommandProductServiceImpl");

    // Build a two-message SQS event: one valid, one with invalid JSON body.
    let valid_eb_event = {
        let mut eb = EventBridgeEvent::<Value>::default();
        eb.id = Some("valid-id".to_owned());
        eb.detail_type = "shopifyWebhook".to_owned();
        eb.source = "aws.partner/shopify.com/test".to_owned();
        eb.detail = real_shopify_eb_detail(SHOPIFY_TOPIC_PRODUCTS_CREATE);
        eb
    };

    let mut valid_msg = SqsMessage::default();
    valid_msg.message_id = Some("valid-msg".to_owned());
    valid_msg.body = Some(serde_json::to_string(&valid_eb_event).unwrap());

    let mut invalid_msg = SqsMessage::default();
    invalid_msg.message_id = Some("invalid-msg".to_owned());
    invalid_msg.body = Some("not valid json {{{".to_owned());

    let mut sqs_event = SqsEvent::default();
    sqs_event.records = vec![valid_msg, invalid_msg];
    let event = LambdaEvent::new(sqs_event, Context::default());

    let result = handler(event, &get_shop_service, &product_service)
        .await
        .unwrap();

    // Only the invalid message should be reported as a failure.
    assert_eq!(
        1,
        result.batch_item_failures.len(),
        "expected exactly one partial failure"
    );
    assert_eq!("invalid-msg", result.batch_item_failures[0].item_identifier);

    // The valid message should have produced a domain event.
    let events = query_events(&product_repo, shop_id).await;
    assert!(
        !events.is_empty(),
        "expected domain events from valid message"
    );
}
