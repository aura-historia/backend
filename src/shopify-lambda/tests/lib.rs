use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::domain::Domain;
use common::price::domain::FixedFxRate;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
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

/// Builds a real-world Shopify EventBridge payload for the given topic.
fn real_shopify_event(topic: &str) -> LambdaEvent<EventBridgeEvent<Value>> {
    let detail = json!({
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
    });

    let mut event = EventBridgeEvent::<Value>::default();
    event.id = Some("33305a23-4886-4909-8a6a-42ef59c41fe2".to_owned());
    event.detail_type = "shopifyWebhook".to_owned();
    event.source = "aws.partner/shopify.com/359227195393/aura-historia-backend-dev".to_owned();
    event.detail = detail;
    LambdaEvent::new(event, Context::default())
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
        url: None,
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
        created: now,
        updated: now,
    };

    repository.put_shop_record(record.clone()).await.unwrap();
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
// The `upsert()` service method writes `ProductEventRecord` entries to DynamoDB.
// A `ProductRecord` (materialized product state) is only created later by the
// product-lambda-materialize-dynamodb Lambda via DynamoDB Streams. Integration
// tests therefore assert on domain event records, not on the materialized record.

#[localstack_test(services = [DynamoDB()])]
async fn should_write_domain_created_event_when_shopify_create_event_with_real_payload() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let fx_rate = FixedFxRate();
    let seller_service = MockSellerService::default();
    let product_service =
        CommandProductServiceImpl::new(&product_repo, &fx_rate, &get_shop_service, &seller_service);

    handler(
        real_shopify_event(SHOPIFY_TOPIC_PRODUCTS_CREATE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

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
async fn should_write_domain_created_event_when_shopify_update_event_with_real_payload() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let fx_rate = FixedFxRate();
    let seller_service = MockSellerService::default();
    let product_service =
        CommandProductServiceImpl::new(&product_repo, &fx_rate, &get_shop_service, &seller_service);

    // Without a materialized ProductRecord, upsert treats the update as a new
    // product and writes a DOMAIN_CREATED event.
    handler(
        real_shopify_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

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

#[localstack_test(services = [DynamoDB()])]
async fn should_write_removed_state_event_when_shopify_delete_event_with_real_payload() {
    let (shop_repo, product_repo) = get_repositories().await;

    let shop_record = seed_shopify_partner_shop(&shop_repo).await;
    let shop_id = shop_record.shop_id;

    let get_shop_service = GetShopServiceImpl::new(&shop_repo);
    let fx_rate = FixedFxRate();
    let seller_service = MockSellerService::default();
    let product_service =
        CommandProductServiceImpl::new(&product_repo, &fx_rate, &get_shop_service, &seller_service);

    // products/delete maps to ProductState::Removed. Without a materialized
    // ProductRecord the delete is processed as a new product creation with
    // state=Removed, producing a DOMAIN_CREATED event whose new_state is Removed.
    handler(
        real_shopify_event(SHOPIFY_TOPIC_PRODUCTS_DELETE),
        &get_shop_service,
        &product_service,
    )
    .await
    .unwrap();

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
