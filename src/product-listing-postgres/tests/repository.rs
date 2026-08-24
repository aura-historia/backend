use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::product::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing, RehydratedProductState,
};
use product_listing_core::product_id::{ProductId, ProductKey};
use product_listing_core::product_image::ProductImage;
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_slug_id::ProductSlugId;
use product_listing_core::product_state::ProductState;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::title::Title;
use product_listing_postgres::{SqlxProductEventStoreFactory, SqlxProductRepositoryFactory};
use product_listing_service::ports::{
    ProductEventStore, ProductEventStoreFactory, ProductRepository, ProductRepositoryError,
    ProductRepositoryFactory,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_append_find_and_update_product_by_id_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-main-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-main-seller").await;
    let product = sample_product("postgres-product-main", shop_id, seller_id);
    let created_event = product.pending_events()[0].clone();

    let mut tx = begin(&unit_of_work).await;
    match products
        .in_transaction(&mut tx)
        .insert(&product, created_event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&created_event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append product event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let Versioned {
        value: loaded_by_id,
        version,
    } = match products
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing product by id"),
        Err(error) => panic!("failed to find product by id: {error:?}"),
    };

    let loaded_by_key = products
        .in_transaction(&mut tx)
        .find_by_key(&ProductKey::new(
            product.shop_id(),
            product.shops_product_id().clone(),
        ))
        .await;
    assert!(matches!(
        loaded_by_key,
        Ok(Some(Versioned { ref value, .. })) if value.id() == product.id()
    ));

    let current_event_id = match events
        .in_transaction(&mut tx)
        .find_current_event_id(product.id())
        .await
    {
        Ok(Some(event_id)) => event_id,
        Ok(None) => panic!("missing current product event id"),
        Err(error) => panic!("failed to find current event id: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(product.id(), loaded_by_id.id());
    assert_eq!(created_event.event_id, version);
    assert_eq!(created_event.event_id, current_event_id);
    assert_eq!(ProductLifecycle::Active, loaded_by_id.lifecycle());

    let mut updated = loaded_by_id;
    let outcome = updated.mark_available();
    assert!(outcome.is_ok());
    let update_event = updated.pending_events()[0].clone();
    let mut tx = begin(&unit_of_work).await;
    match products
        .in_transaction(&mut tx)
        .update(&updated, version, update_event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to update product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&update_event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append update event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = match products
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing updated product"),
        Err(error) => panic!("failed to load updated product: {error:?}"),
    };
    let current_event_id_after_update = match events
        .in_transaction(&mut tx)
        .find_current_event_id(product.id())
        .await
    {
        Ok(Some(event_id)) => event_id,
        Ok(None) => panic!("missing current product event id after update"),
        Err(error) => panic!("failed to find current event id after update: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(ProductState::Available, loaded.value.state());
    assert_eq!(update_event.event_id, loaded.version);
    assert_eq!(update_event.event_id, current_event_id_after_update);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_round_trip_immutable_sale_valuation_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-sale-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-sale-seller").await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;
    let valuation = product_listing_core::product::ProductSaleValuation {
        sold_at: OffsetDateTime::UNIX_EPOCH,
        fx_rate_id,
    };
    let mut product = sample_product("postgres-product-sale", shop_id, seller_id);
    let transition = product.mark_sold(valuation);
    assert!(matches!(
        transition,
        Ok(domain_primitives::change_outcome::ChangeOutcome::Changed)
    ));
    let current_event_id = match product.pending_events().last() {
        Some(event) => event.event_id,
        None => panic!("sold product is missing events"),
    };

    let mut tx = begin(&unit_of_work).await;
    match products
        .in_transaction(&mut tx)
        .insert(&product, current_event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert sold product: {error:?}"),
    }
    for event in product.pending_events() {
        match events.in_transaction(&mut tx).append(event).await {
            Ok(()) => {}
            Err(error) => panic!("failed to append sold product event: {error:?}"),
        }
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = match products
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("persisted sold product is missing"),
        Err(error) => panic!("failed to load sold product: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(ProductState::Sold, loaded.value.state());
    assert_eq!(Some(valuation), loaded.value.sale_valuation());
    assert_eq!(product.pricing(), loaded.value.pricing());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_enforce_product_sale_valuation_state_constraints_in_postgres() {
    let pool = get_postgres_client().await;
    let shop_id = seed_shop(&pool, "product-listing-postgres-sale-constraint-shop").await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;

    let available_with_sale = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-available-with-sale",
        ProductState::Available,
        Some(fx_rate_id),
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    assert_check_violation(available_with_sale);

    let sold_without_sale = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-sold-without-sale",
        ProductState::Sold,
        None,
        None,
    )
    .await;
    assert_check_violation(sold_without_sale);

    let removed_with_sale = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-removed-with-sale",
        ProductState::Removed,
        Some(fx_rate_id),
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    if let Err(error) = removed_with_sale {
        panic!("removed Product with sale valuation must be valid: {error}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_product_is_missing_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let products = SqlxProductRepositoryFactory::new();
    let product_id = ProductId::new();

    let mut tx = begin(&unit_of_work).await;
    let by_id = match products
        .in_transaction(&mut tx)
        .find_by_id(product_id)
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing product by id: {error:?}"),
    };

    commit(tx).await;

    assert!(by_id.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_insert_conflict_when_shop_product_identity_or_slug_duplicates() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-conflict-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-conflict-seller").await;
    let first = sample_product("postgres-product-conflict", shop_id, seller_id);
    let second = sample_product("postgres-product-conflict", shop_id, seller_id);

    insert_product_with_event(&unit_of_work, &products, &events, &first).await;

    let mut tx = begin(&unit_of_work).await;
    let duplicate_product = products
        .in_transaction(&mut tx)
        .insert(&second, EventId::new())
        .await;
    assert!(matches!(
        duplicate_product,
        Err(ProductRepositoryError::ShopProductAlreadyExists)
            | Err(ProductRepositoryError::ProductSlugAlreadyExists)
            | Err(ProductRepositoryError::ProductInsertFailed)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_product_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-missing-update-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-missing-update-seller").await;
    let product = sample_product("postgres-product-missing-update", shop_id, seller_id);

    let result = {
        let mut tx = begin(&unit_of_work).await;
        products
            .in_transaction(&mut tx)
            .update(&product, EventId::new(), EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductRepositoryError::ProductCurrentEventIdConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_event_id_is_stale() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-stale-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-stale-seller").await;
    let mut product = sample_product("postgres-product-stale", shop_id, seller_id);

    insert_product_with_event(&unit_of_work, &products, &events, &product).await;

    let outcome = product.mark_reserved();
    assert!(outcome.is_ok());
    let result = {
        let mut tx = begin(&unit_of_work).await;
        products
            .in_transaction(&mut tx)
            .update(&product, EventId::new(), EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductRepositoryError::ProductCurrentEventIdConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_identity_conflict_when_update_would_duplicate_shop_product_identity() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-update-key-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-update-key-seller").await;
    let first = sample_product("postgres-product-update-key-first", shop_id, seller_id);
    let second = sample_product("postgres-product-update-key-second", shop_id, seller_id);
    insert_product_with_event(&unit_of_work, &products, &events, &first).await;
    insert_product_with_event(&unit_of_work, &products, &events, &second).await;
    let conflict = rehydrate_product_for_update(
        &second,
        second.slug_id().clone(),
        first.shop_id(),
        first.shops_product_id().clone(),
    );

    let result = {
        let mut tx = begin(&unit_of_work).await;
        products
            .in_transaction(&mut tx)
            .update(
                &conflict,
                second.pending_events()[0].event_id,
                EventId::new(),
            )
            .await
    };

    assert!(matches!(
        result,
        Err(ProductRepositoryError::ShopProductAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_slug_conflict_when_update_would_duplicate_product_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-update-slug-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-update-slug-seller").await;
    let first = sample_product("postgres-product-update-slug-first", shop_id, seller_id);
    let second = sample_product("postgres-product-update-slug-second", shop_id, seller_id);
    insert_product_with_event(&unit_of_work, &products, &events, &first).await;
    insert_product_with_event(&unit_of_work, &products, &events, &second).await;
    let conflict = rehydrate_product_for_update(
        &second,
        first.slug_id().clone(),
        second.shop_id(),
        second.shops_product_id().clone(),
    );

    let result = {
        let mut tx = begin(&unit_of_work).await;
        products
            .in_transaction(&mut tx)
            .update(
                &conflict,
                second.pending_events()[0].event_id,
                EventId::new(),
            )
            .await
    };

    assert!(matches!(
        result,
        Err(ProductRepositoryError::ProductSlugAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_roll_back_product_and_event_when_transaction_is_not_committed() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let products = SqlxProductRepositoryFactory::new();
    let events = SqlxProductEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-rollback-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-rollback-seller").await;
    let product = sample_product("postgres-product-rollback", shop_id, seller_id);
    let event = product.pending_events()[0].clone();

    {
        let mut tx = begin(&unit_of_work).await;
        match products
            .in_transaction(&mut tx)
            .insert(&product, event.event_id)
            .await
        {
            Ok(_) => {}
            Err(error) => panic!("failed to insert product before rollback: {error:?}"),
        }
        match events.in_transaction(&mut tx).append(&event).await {
            Ok(_) => {}
            Err(error) => panic!("failed to append event before rollback: {error:?}"),
        }
    }

    let mut tx = begin(&unit_of_work).await;
    let product_after_rollback = match products
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find rolled-back product: {error:?}"),
    };
    let current_event_after_rollback = match events
        .in_transaction(&mut tx)
        .find_current_event_id(product.id())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find rolled-back current event: {error:?}"),
    };
    let persisted_event_count: i64 =
        match sqlx::query_scalar("SELECT count(*) FROM product_events WHERE product_id = $1")
            .bind(uuid::Uuid::from(product.id()))
            .fetch_one(&pool)
            .await
        {
            Ok(value) => value,
            Err(error) => panic!("failed to count rolled-back events: {error}"),
        };
    commit(tx).await;

    assert!(product_after_rollback.is_none());
    assert_eq!(None, current_event_after_rollback);
    assert_eq!(0, persisted_event_count);
}

async fn insert_product_row(
    pool: &sqlx::PgPool,
    shop_id: ShopId,
    slug: &str,
    state: ProductState,
    sale_fx_rate_id: Option<FxRateId>,
    sold_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    let product_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, state, lifecycle, url, sale_fx_rate_id, sold_at) VALUES ($1, $2, $3, $4, $4, $5, $6, 'ACTIVE', 'https://example.test/product', $7, $8)",
    )
    .bind(product_id)
    .bind(slug)
    .bind(event_id)
    .bind(uuid::Uuid::from(shop_id))
    .bind(format!("{slug}-sku"))
    .bind(match state {
        ProductState::Listed => "LISTED",
        ProductState::Available => "AVAILABLE",
        ProductState::Reserved => "RESERVED",
        ProductState::Sold => "SOLD",
        ProductState::Removed => "REMOVED",
        ProductState::Unknown => "UNKNOWN",
    })
    .bind(sale_fx_rate_id.map(uuid::Uuid::from))
    .bind(sold_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', 'DOMAIN', '{}', now())",
    )
    .bind(event_id)
    .bind(product_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

fn assert_check_violation(result: Result<(), sqlx::Error>) {
    assert!(matches!(
        result,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23514")
    ));
}

async fn insert_product_with_event(
    unit_of_work: &SqlxUnitOfWork,
    products: &SqlxProductRepositoryFactory,
    events: &SqlxProductEventStoreFactory,
    product: &Product,
) {
    let event = product.pending_events()[0].clone();
    let mut tx = begin(unit_of_work).await;
    match products
        .in_transaction(&mut tx)
        .insert(product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append product event: {error:?}"),
    }
    commit(tx).await;
}

fn rehydrate_product_for_update(
    product: &Product,
    slug_id: ProductSlugId,
    shop_id: ShopId,
    shops_product_id: product_listing_core::shops_product_id::ShopsProductId,
) -> Product {
    match Product::rehydrate(RehydratedProductState {
        id: product.id(),
        slug_id,
        shop_id,
        seller_id: product.seller_id(),
        shops_product_id,
        address: product.address(),
        title: product.title().cloned(),
        description: product.description().cloned(),
        pricing: product.pricing(),
        sale_valuation: product.sale_valuation(),
        state: product.state(),
        lifecycle: product.lifecycle(),
        url: product.url().clone(),
        images: product.images().clone(),
        auction: product.auction(),
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to rehydrate conflict product: {error:?}"),
    }
}

fn sample_product(slug: &str, shop_id: ShopId, seller_id: ShopId) -> Product {
    let mut images = IndexSet::new();
    images.insert(ProductImage {
        url: url(&format!("https://example.com/{slug}.jpg")),
        prohibited_content: ProhibitedContent::None,
    });
    match Product::create(NewProduct {
        id: product_listing_core::product_id::ProductId::new(),
        shop_id,
        seller_id,
        shops_product_id: product_listing_core::shops_product_id::ShopsProductId::from(slug),
        address: ProductAddress::default(),
        title: Some(Localized::new(Language::En, Title::from(slug))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Nice product"),
        )),
        pricing: ProductPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        sale_valuation: None,
        state: ProductState::Listed,
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductAuction::default(),
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

async fn seed_complete_fx_snapshot(pool: &sqlx::PgPool, fx_rate_id: FxRateId) {
    let rate = sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind(OffsetDateTime::UNIX_EPOCH)
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await;
    if let Err(error) = rate {
        panic!("failed to seed FX snapshot: {error}");
    }

    for currency in Currency::iter() {
        let quote = sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency.as_str())
        .bind(if currency == Currency::Eur {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await;
        if let Err(error) = quote {
            panic!("failed to seed FX quote: {error}");
        }
    }
}

async fn seed_shop(pool: &sqlx::PgPool, slug: &str) -> ShopId {
    let shop_id = ShopId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(slug)
    .bind(ShopName::from(slug).to_string())
    .bind("COMMERCIAL_DEALER")
    .bind("SCRAPED")
    .bind(Vec::<String>::from([format!("{slug}.example")]))
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed shop: {error}");
    }

    shop_id
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
