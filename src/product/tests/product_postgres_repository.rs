use common::language::domain::Language;
use common::localized::Localized;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use product::core::product::Product;
use product::core::product_event::{ProductDomainEvent, ProductEvent, ProductEventPayload};
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::postgres::{
    ProductEventGroup, ProductPostgresRepository, ProductPostgresRepositoryError,
};
use serial_test::serial;
use shop::core::shop_type::ShopType;
use sqlx::PgPool;
use test_api::*;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[serial]
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_product_and_event_in_one_transaction() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let event_record = created_event_record(shop_id, "external-1");
    let product_id = event_record.aggregate_id;
    let event_id = event_record.event_id;
    let repository = ProductPostgresRepository::new(pool.clone());

    repository
        .insert_created_product(event_record)
        .await
        .unwrap();

    let product = repository
        .get_product(&common::product_id::ProductKey::new(
            shop_id,
            ShopsProductId::from("external-1"),
        ))
        .await
        .unwrap()
        .unwrap();
    let events = repository
        .list_events_for_product(product_id)
        .await
        .unwrap();

    assert_eq!(product_id, product.product_id);
    assert_eq!(event_id, product.event_id);
    assert_eq!(ProductState::Listed, product.state);
    assert_eq!(2, product.images.len());
    assert_eq!(
        vec![
            "https://cdn.example.com/one.jpg",
            "https://cdn.example.com/two.jpg"
        ],
        product
            .images
            .iter()
            .map(|image| image.url.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(1, events.len());
    assert_eq!(ProductEventGroup::Domain, events[0].event_group);
    assert_eq!("DOMAIN_CREATED", events[0].event_type);
    assert!(events[0].payload.get("product_slug_id").is_some());
}

#[serial]
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_update_product_and_append_event_in_one_transaction() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let event_record = created_event_record(shop_id, "external-2");
    let product_id = event_record.aggregate_id;
    let repository = ProductPostgresRepository::new(pool.clone());
    repository
        .insert_created_product(event_record)
        .await
        .unwrap();
    let key = common::product_id::ProductKey::new(shop_id, ShopsProductId::from("external-2"));
    let mut product = repository.get_product(&key).await.unwrap().unwrap();
    let expected_event_id = product.event_id;
    let update_event = product.change_state(ProductState::Sold).unwrap();
    let update_event_id = update_event.event_id;

    repository
        .update_product_with_events(product, vec![domain_event(update_event)], expected_event_id)
        .await
        .unwrap();

    let product = repository.get_product(&key).await.unwrap().unwrap();
    let events = repository
        .list_events_for_product(product_id)
        .await
        .unwrap();

    assert_eq!(ProductState::Sold, product.state);
    assert_eq!(update_event_id, product.event_id);
    assert_eq!(2, events.len());
    assert_eq!("DOMAIN_STATE_CHANGED", events[1].event_type);
}

#[serial]
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_update_product_and_append_delete_event_in_one_transaction() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let event_record = created_event_record(shop_id, "external-delete");
    let product_id = event_record.aggregate_id;
    let repository = ProductPostgresRepository::new(pool.clone());
    repository
        .insert_created_product(event_record)
        .await
        .unwrap();
    let key = common::product_id::ProductKey::new(shop_id, ShopsProductId::from("external-delete"));
    let mut product = repository.get_product(&key).await.unwrap().unwrap();
    let expected_event_id = product.event_id;
    let delete_event = product.delete().unwrap();
    let delete_event_id = delete_event.event_id;

    repository
        .update_product_with_events(
            product,
            vec![lifecycle_event(delete_event)],
            expected_event_id,
        )
        .await
        .unwrap();

    let product = repository.get_product(&key).await.unwrap().unwrap();
    let events = repository
        .list_events_for_product(product_id)
        .await
        .unwrap();

    assert_eq!(ProductLifecycle::Deleted, product.lifecycle);
    assert_eq!(delete_event_id, product.event_id);
    assert_eq!(2, events.len());
    assert_eq!(ProductEventGroup::Lifecycle, events[1].event_group);
    assert_eq!("LIFECYCLE_DELETED", events[1].event_type);
}

#[serial]
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_update_when_expected_event_id_is_stale() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;
    let event_record = created_event_record(shop_id, "external-3");
    let product_id = event_record.aggregate_id;
    let repository = ProductPostgresRepository::new(pool.clone());
    repository
        .insert_created_product(event_record)
        .await
        .unwrap();
    let key = common::product_id::ProductKey::new(shop_id, ShopsProductId::from("external-3"));
    let mut product = repository.get_product(&key).await.unwrap().unwrap();
    let update_event = product.change_state(ProductState::Sold).unwrap();

    let error = repository
        .update_product_with_events(
            product,
            vec![domain_event(update_event)],
            common::event_id::EventId::new(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ProductPostgresRepositoryError::ConcurrentModification
    ));
    let events = repository
        .list_events_for_product(product_id)
        .await
        .unwrap();
    assert_eq!(1, events.len());
}

fn domain_event(event: ProductDomainEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductDomainEvent(event.payload),
    }
}

fn lifecycle_event(event: product::core::product_event::ProductLifecycleEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductLifecycleEvent(event.payload),
    }
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

fn created_event_record(shop_id: ShopId, shops_product_id: &str) -> ProductDomainEvent {
    Product::create(
        shop_id,
        shop_id,
        ShopsProductId::from(shops_product_id),
        ShopName::from("Shop One"),
        ShopName::from("Shop One"),
        ShopType::AuctionHouse,
        None,
        None,
        Localized::new(Language::En, Title::from("A vase")),
        None,
        None,
        Default::default(),
        None,
        Default::default(),
        None,
        Default::default(),
        ProductState::Listed,
        Url::parse("https://shop.example.com/products/external").unwrap(),
        Url::parse("https://aura.example.com/shops/shop-one/products/product-one").unwrap(),
        [
            ProductImage {
                url: Url::parse("https://cdn.example.com/one.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
            ProductImage {
                url: Url::parse("https://cdn.example.com/two.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
        ],
        None,
        None,
    )
}
