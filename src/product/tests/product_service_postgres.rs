use common::actor::domain::Actor;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::FixedFxRate;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use fxrate::dynamodb::record::FxRatesRecord;
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::postgres::ProductPostgresRepository;
use product::service::product_command::{CreateProductCommand, UpdateProductCommand};
use product::service::product_service::{ProductService, ProductServiceImpl};
use serial_test::serial;
use shop::core::partner_status::ShopPartnerStatus;
use shop::core::shop::Shop;
use shop::core::shop_type::ShopType;
use shop::service::get_service::MockGetShopService;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use test_api::*;
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[serial]
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_facade_create_update_and_delete_product_in_postgres() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let repository = ProductPostgresRepository::new(pool.clone());
    let shop_service = shop_service(shop_id);
    let service = ProductServiceImpl::new_with_fx_rate(
        &repository,
        FxRatesRecord::from(FixedFxRate()),
        &shop_service,
    );
    let key =
        common::product_id::ProductKey::new(shop_id, ShopsProductId::from("external-service"));

    let create_failures = service.create(vec![create_command(shop_id)]).await;
    assert!(create_failures.is_empty());

    let product = repository.get_product(&key).await.unwrap().unwrap();
    assert_eq!(ProductState::Listed, product.state);

    let mut updates = HashMap::new();
    updates.insert(
        key.clone(),
        UpdateProductCommand {
            state: Some(ProductState::Sold),
            ..Default::default()
        },
    );
    let update_failures = service.update(updates).await;
    assert!(update_failures.is_empty());

    let product = repository.get_product(&key).await.unwrap().unwrap();
    assert_eq!(ProductState::Sold, product.state);

    service.delete(&key).await.unwrap();

    let product = repository.get_product(&key).await.unwrap().unwrap();
    let events = repository
        .list_events_for_product(product.product_id)
        .await
        .unwrap();
    assert_eq!(ProductLifecycle::Deleted, product.lifecycle);
    assert_eq!(3, events.len());
    assert_eq!("DOMAIN_CREATED", events[0].event_type);
    assert_eq!("DOMAIN_STATE_CHANGED", events[1].event_type);
    assert_eq!("LIFECYCLE_DELETED", events[2].event_type);
}

fn shop_service(shop_id: ShopId) -> MockGetShopService {
    let mut service = MockGetShopService::default();
    service.expect_find_shop().returning(move |_| {
        let shop = Shop {
            shop_id,
            shop_slug_id: ShopSlugId::from("Shop One"),
            name: ShopName::from("Shop One"),
            shop_type: ShopType::AuctionHouse,
            domains: HashSet::new(),
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_webhook_secret: None,
            woocommerce_currency: None,
            woocommerce_language: None,
            url: None,
            view_url: None,
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status: ShopPartnerStatus::Partnered,
            affiliate_configuration: None,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };
        Box::pin(async move { Ok(shop) })
    });
    service
}

async fn insert_shop(pool: &PgPool, shop_id: ShopId) {
    sqlx::query(
        "INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(format!("shop-{shop_id}"))
    .bind("Shop One")
    .bind("AUCTION_HOUSE")
    .bind("PARTNERED")
    .bind("SYSTEM")
    .bind("SYSTEM")
    .execute(pool)
    .await
    .unwrap();
}

fn create_command(shop_id: ShopId) -> CreateProductCommand {
    CreateProductCommand {
        shop_id,
        shops_product_id: ShopsProductId::from("external-service"),
        seller_name_raw: None,
        structured_address: None,
        geo_address: None,
        native_title: Localized::new(Language::En, Title::from("A vase")),
        other_title: HashMap::new(),
        native_description: None,
        native_price: None,
        other_price: HashMap::new(),
        native_price_estimate_min: None,
        other_price_estimate_min: HashMap::new(),
        native_price_estimate_max: None,
        other_price_estimate_max: HashMap::new(),
        state: ProductState::Listed,
        url: Url::parse("https://shop.example.com/products/external-service").unwrap(),
        images: [ProductImage {
            url: Url::parse("https://cdn.example.com/service.jpg").unwrap(),
            prohibited_content: ProhibitedContent::Unknown,
        }]
        .into(),
        auction_start: None,
        auction_end: None,
    }
}
