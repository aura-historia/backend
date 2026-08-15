use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductKey;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::versioned::Versioned;
use indexmap::IndexSet;
use product::core::description::Description;
use product::core::fx_rate_id::FxRateId;
use product::core::product_aggregate::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing,
};
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::postgres::event_store::SqlxProductEventStore;
use product::postgres::repository::SqlxProductRepository;
use product::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use product::service::ports::product_repository::{ProductRepository, ProductRepositoryError};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_product_and_event_in_same_transaction() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    let seller_id = ShopId::new();
    let fx_rate_id = FxRateId::new();
    seed_shop(&pool, shop_id, "product-postgres-shop").await;
    seed_shop(&pool, seller_id, "product-postgres-seller").await;
    seed_fx_rate(&pool, fx_rate_id).await;
    let product = create_product(shop_id, seller_id, Some(fx_rate_id), "insert");
    let event_id = last_pending_event_id(&product);

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    match SqlxProductRepository::new(&mut tx)
        .insert(&product, event_id)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    for event in product.pending_events() {
        match SqlxProductEventStore::new(&mut tx).append(event).await {
            Ok(()) => {}
            Err(error) => panic!("failed to append product event: {error:?}"),
        }
    }
    match tx.commit().await {
        Ok(()) => {}
        Err(error) => panic!("failed to commit transaction: {error}"),
    }

    let loaded = load_product(&pool, product.id()).await;
    assert_eq!(product.id(), loaded.value.id());
    assert_eq!(event_id, loaded.version);
    assert_eq!(Some(fx_rate_id), loaded.value.pricing().fx_rate_id);
    assert_eq!(1, event_count(&pool, product.id()).await);
    assert_eq!(Some(event_id), current_event_id(&pool, product.id()).await);
    assert_eq!(
        Some("created".to_owned()),
        event_payload_kind(&pool, event_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_update_product_event_id_and_append_update_event() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    let seller_id = ShopId::new();
    seed_shop(&pool, shop_id, "product-postgres-update-shop").await;
    seed_shop(&pool, seller_id, "product-postgres-update-seller").await;
    let product = create_product(shop_id, seller_id, None, "update");
    persist_created_product(&pool, &product).await;
    let created_event_id = last_pending_event_id(&product);

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    let loaded = match SqlxProductRepository::new(&mut tx)
        .find_by_key(&ProductKey::new(
            shop_id,
            product.shops_product_id().clone(),
        ))
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("product not found by key"),
        Err(error) => panic!("failed to load product by key: {error:?}"),
    };
    let expected_event_id = loaded.version;
    let mut updated = loaded.value;
    updated.change_state(ProductState::Available);
    let events = updated.take_pending_events();
    let updated_event_id = match events.last() {
        Some(event) => event.event_id,
        None => panic!("missing update event"),
    };

    match SqlxProductRepository::new(&mut tx)
        .update(&updated, expected_event_id, updated_event_id)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to update product: {error:?}"),
    }
    for event in &events {
        match SqlxProductEventStore::new(&mut tx).append(event).await {
            Ok(()) => {}
            Err(error) => panic!("failed to append update event: {error:?}"),
        }
    }
    match tx.commit().await {
        Ok(()) => {}
        Err(error) => panic!("failed to commit update: {error}"),
    }

    let loaded = load_product(&pool, product.id()).await;
    assert_eq!(ProductState::Available, loaded.value.state());
    assert_eq!(updated_event_id, loaded.version);
    assert_ne!(created_event_id, updated_event_id);
    assert_eq!(2, event_count(&pool, product.id()).await);
    assert_eq!(
        Some(updated_event_id),
        current_event_id(&pool, product.id()).await
    );
    assert_eq!(
        Some("stateChanged".to_owned()),
        event_payload_kind(&pool, updated_event_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_current_event_id() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    let seller_id = ShopId::new();
    seed_shop(&pool, shop_id, "product-postgres-stale-shop").await;
    seed_shop(&pool, seller_id, "product-postgres-stale-seller").await;
    let product = create_product(shop_id, seller_id, None, "stale");
    persist_created_product(&pool, &product).await;
    let current = last_pending_event_id(&product);
    let stale = EventId::new();
    let replacement = EventId::new();

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    let result = SqlxProductRepository::new(&mut tx)
        .update(&product, stale, replacement)
        .await;
    match tx.rollback().await {
        Ok(()) => {}
        Err(error) => panic!("failed to rollback stale update: {error}"),
    }

    assert!(matches!(
        result,
        Err(ProductRepositoryError::ProductCurrentEventIdConflict)
    ));
    assert_eq!(Some(current), current_event_id(&pool, product.id()).await);
    assert_eq!(1, event_count(&pool, product.id()).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_product_when_event_append_fails() {
    let pool = get_postgres_client().await;
    let shop_id = ShopId::new();
    let seller_id = ShopId::new();
    seed_shop(&pool, shop_id, "product-postgres-rollback-shop").await;
    seed_shop(&pool, seller_id, "product-postgres-rollback-seller").await;
    let product = create_product(shop_id, seller_id, None, "rollback");
    let event_id = last_pending_event_id(&product);
    let event = match product.pending_events().first() {
        Some(event) => event,
        None => panic!("missing created event"),
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    };
    match SqlxProductRepository::new(&mut tx)
        .insert(&product, event_id)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to insert product before rollback: {error:?}"),
    }
    match SqlxProductEventStore::new(&mut tx).append(event).await {
        Ok(()) => {}
        Err(error) => panic!("failed to append first event before rollback: {error:?}"),
    }
    let duplicate_result = SqlxProductEventStore::new(&mut tx).append(event).await;
    let _ = tx.rollback().await;

    assert!(matches!(
        duplicate_result,
        Err(ProductEventStoreError::ProductEventAlreadyExists)
    ));
    assert_eq!(None, current_event_id(&pool, product.id()).await);
    assert_eq!(0, event_count(&pool, product.id()).await);
}

fn create_product(
    shop_id: ShopId,
    seller_id: ShopId,
    fx_rate_id: Option<FxRateId>,
    suffix: &str,
) -> Product {
    match Product::create(NewProduct {
        id: Default::default(),
        shop_id,
        seller_id,
        shops_product_id: ShopsProductId::from(format!("external-{suffix}")),
        address: ProductAddress::default(),
        title: Some(Localized::new(
            Language::En,
            Title::from(format!("Bronze vase {suffix}")),
        )),
        description: Some(Localized::new(
            Language::En,
            Description::from(format!("Native description {suffix}")),
        )),
        pricing: ProductPricing {
            native_price: Some(Price::new(MonetaryAmount::from(1_500_u64), Currency::Eur)),
            native_price_estimate_min: Some(Price::new(
                MonetaryAmount::from(1_000_u64),
                Currency::Eur,
            )),
            native_price_estimate_max: Some(Price::new(
                MonetaryAmount::from(2_000_u64),
                Currency::Eur,
            )),
            fx_rate_id,
        },
        state: ProductState::Listed,
        url: parse_url(&format!("https://shop.example/products/{suffix}")),
        images: product_images(suffix),
        auction: ProductAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH),
            end: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::days(1)),
        },
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

fn product_images(suffix: &str) -> IndexSet<ProductImage> {
    let mut images = IndexSet::new();
    images.insert(ProductImage {
        url: parse_url(&format!("https://cdn.example/{suffix}.jpg")),
        prohibited_content: ProhibitedContent::None,
    });
    images
}

fn parse_url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

fn last_pending_event_id(product: &Product) -> EventId {
    match product.pending_events().last() {
        Some(event) => event.event_id,
        None => panic!("missing pending event"),
    }
}

async fn seed_shop(pool: &sqlx::PgPool, shop_id: ShopId, slug: &str) {
    let result = sqlx::query(
        r#"
        INSERT INTO shops (
            shop_id, shop_slug_id, name, shop_type, partner_status, created_by, updated_by
        ) VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'PARTNERED', 'product-test', 'product-test')
        "#,
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(slug)
    .bind(slug)
    .execute(pool)
    .await;
    if let Err(error) = result {
        panic!("failed to seed shop: {error}");
    }
}

async fn seed_fx_rate(pool: &sqlx::PgPool, fx_rate_id: FxRateId) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin fx transaction: {error}"),
    };
    let insert_rate = sqlx::query(
        r#"
        INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id)
        VALUES ($1, now(), 'fxratesapi', $2)
        "#,
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind(fx_rate_id.to_string())
    .execute(&mut *tx)
    .await;
    if let Err(error) = insert_rate {
        panic!("failed to seed fx rate: {error}");
    }

    if let Err(error) = tx.commit().await {
        panic!("failed to commit fx seed: {error}");
    }
}

async fn persist_created_product(pool: &sqlx::PgPool, product: &Product) {
    let event_id = last_pending_event_id(product);
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin create transaction: {error}"),
    };
    match SqlxProductRepository::new(&mut tx)
        .insert(product, event_id)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to insert created product: {error:?}"),
    }
    for event in product.pending_events() {
        match SqlxProductEventStore::new(&mut tx).append(event).await {
            Ok(()) => {}
            Err(error) => panic!("failed to append created event: {error:?}"),
        }
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit created product: {error}");
    }
}

async fn load_product(
    pool: &sqlx::PgPool,
    product_id: common::product_id::ProductId,
) -> Versioned<Product, EventId> {
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection,
        Err(error) => panic!("failed to acquire connection: {error}"),
    };
    match SqlxProductRepository::new(&mut connection)
        .find_by_id(product_id)
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("product not found"),
        Err(error) => panic!("failed to load product: {error:?}"),
    }
}

async fn current_event_id(
    pool: &sqlx::PgPool,
    product_id: common::product_id::ProductId,
) -> Option<EventId> {
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection,
        Err(error) => panic!("failed to acquire connection: {error}"),
    };
    match SqlxProductEventStore::new(&mut connection)
        .find_current_event_id(product_id)
        .await
    {
        Ok(event_id) => event_id,
        Err(error) => panic!("failed to find current event id: {error:?}"),
    }
}

async fn event_count(pool: &sqlx::PgPool, product_id: common::product_id::ProductId) -> i64 {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM product_events WHERE product_id = $1")
        .bind(uuid::Uuid::from(product_id))
        .fetch_one(pool)
        .await
    {
        Ok(count) => count,
        Err(error) => panic!("failed to count product events: {error}"),
    }
}

async fn event_payload_kind(pool: &sqlx::PgPool, event_id: EventId) -> Option<String> {
    match sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'kind' FROM product_events WHERE event_id = $1",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_optional(pool)
    .await
    {
        Ok(kind) => kind.flatten(),
        Err(error) => panic!("failed to read event payload kind: {error}"),
    }
}
