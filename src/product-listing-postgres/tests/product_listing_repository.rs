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
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    ListingSaleObservation, NewProductListing, ProductListing, ProductListingAddress,
    ProductListingAuction, ProductListingPricing, RehydratedProductListingState,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxProductListingEventStoreFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventStore, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
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
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-main-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-main-seller").await;
    let product = sample_product("postgres-product-main", shop_id, seller_id);
    let created_event = first_stamped_event(&product);

    let mut tx = begin(&unit_of_work).await;
    match product_listings
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
    } = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing product by id"),
        Err(error) => panic!("failed to find product by id: {error:?}"),
    };

    let loaded_by_key = product_listings
        .in_transaction(&mut tx)
        .find_by_key(&ProductListingKey::new(
            product.shop_id(),
            product.shop_listing_id().clone(),
        ))
        .await;
    assert!(matches!(
        loaded_by_key,
        Ok(Some(Versioned { ref value, .. })) if value.id() == product.id()
    ));

    commit(tx).await;

    assert_eq!(product.id(), loaded_by_id.id());
    assert_eq!(created_event.event_id, version);

    assert_eq!(ListingLifecycle::Active, loaded_by_id.lifecycle());

    let mut updated = loaded_by_id;
    let outcome = updated.clear_price();
    assert!(matches!(
        outcome,
        Ok(domain_primitives::change_outcome::ChangeOutcome::Changed)
    ));
    let update_event = first_stamped_event(&updated);
    let mut tx = begin(&unit_of_work).await;
    match product_listings
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
    let loaded = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing updated product"),
        Err(error) => panic!("failed to load updated product: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(None, loaded.value.pricing().price);
    assert_eq!(update_event.event_id, loaded.version);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_round_trip_immutable_sale_observation_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-sale-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-sale-seller").await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;
    let observation = ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, fx_rate_id);
    let mut product = sample_product("postgres-product-sale", shop_id, seller_id);
    let transition = product.record_sale_observation(observation);
    assert!(matches!(
        transition,
        Ok(domain_primitives::change_outcome::ChangeOutcome::Changed)
    ));
    let observation_events = stamp_product_listing_events(
        product.id(),
        OffsetDateTime::now_utc(),
        product.pending_event_payloads().to_vec(),
    );
    let current_event_id = match observation_events.last() {
        Some(event) => event.event_id,
        None => panic!("listing with a sale observation is missing events"),
    };

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, current_event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert listing with sale observation: {error:?}"),
    }
    for event in &observation_events {
        match events.in_transaction(&mut tx).append(event).await {
            Ok(()) => {}
            Err(error) => panic!("failed to append sold product event: {error:?}"),
        }
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("persisted sold product is missing"),
        Err(error) => panic!("failed to load sold product: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(Some(observation), loaded.value.sale_observation());
    assert_eq!(product.pricing(), loaded.value.pricing());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_enforce_paired_sale_observation_columns_in_postgres() {
    let pool = get_postgres_client().await;
    let shop_id = seed_shop(
        &pool,
        "product-listing-postgres-sale-observation-constraint-shop",
    )
    .await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;

    let observation_without_fx_rate = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-observation-without-fx-rate",
        None,
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    assert_check_violation(observation_without_fx_rate);

    let fx_rate_without_observation = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-fx-rate-without-observation",
        Some(fx_rate_id),
        None,
    )
    .await;
    assert_check_violation(fx_rate_without_observation);

    let complete_observation = insert_product_row(
        &pool,
        shop_id,
        "product-listing-postgres-complete-observation",
        Some(fx_rate_id),
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    if let Err(error) = complete_observation {
        panic!("complete sale observation must be valid: {error}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_product_is_missing_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let product_listing_id = ProductListingId::new();

    let mut tx = begin(&unit_of_work).await;
    let by_id = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product_listing_id)
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing product by id: {error:?}"),
    };

    commit(tx).await;

    assert!(by_id.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_insert_conflict_when_shop_product_listing_identity_or_slug_duplicates()
 {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-conflict-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-conflict-seller").await;
    let first = sample_product("postgres-product-conflict", shop_id, seller_id);
    let second = sample_product("postgres-product-conflict", shop_id, seller_id);

    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;

    let mut tx = begin(&unit_of_work).await;
    let duplicate_product = product_listings
        .in_transaction(&mut tx)
        .insert(&second, EventId::new())
        .await;
    assert!(matches!(
        duplicate_product,
        Err(ProductListingRepositoryError::ShopListingAlreadyExists)
            | Err(ProductListingRepositoryError::ProductListingSlugAlreadyExists)
            | Err(ProductListingRepositoryError::ProductListingInsertFailed)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_product_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-missing-update-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-missing-update-seller").await;
    let product = sample_product("postgres-product-missing-update", shop_id, seller_id);

    let result = {
        let mut tx = begin(&unit_of_work).await;
        product_listings
            .in_transaction(&mut tx)
            .update(&product, EventId::new(), EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ProductListingCurrentEventIdConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_event_id_is_stale() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-stale-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-stale-seller").await;
    let mut product = sample_product("postgres-product-stale", shop_id, seller_id);

    insert_product_with_event(&unit_of_work, &product_listings, &events, &product).await;

    let outcome = product.set_availability(ListingAvailability::Reserved);
    assert!(outcome.is_ok());
    let result = {
        let mut tx = begin(&unit_of_work).await;
        product_listings
            .in_transaction(&mut tx)
            .update(&product, EventId::new(), EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ProductListingCurrentEventIdConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_identity_conflict_when_update_would_duplicate_shop_product_listing_identity()
{
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-update-key-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-update-key-seller").await;
    let first = sample_product("postgres-product-update-key-first", shop_id, seller_id);
    let second = sample_product("postgres-product-update-key-second", shop_id, seller_id);
    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;
    insert_product_with_event(&unit_of_work, &product_listings, &events, &second).await;
    let conflict = rehydrate_product_for_update(
        &second,
        second.slug_id().clone(),
        first.shop_id(),
        first.shop_listing_id().clone(),
    );

    let result = {
        let mut tx = begin(&unit_of_work).await;
        let expected_event_id = match product_listings
            .in_transaction(&mut tx)
            .find_by_id(second.id())
            .await
        {
            Ok(Some(loaded)) => loaded.version,
            Ok(None) => panic!("missing second listing"),
            Err(error) => panic!("failed to find second listing: {error:?}"),
        };
        product_listings
            .in_transaction(&mut tx)
            .update(&conflict, expected_event_id, EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ShopListingAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_slug_conflict_when_update_would_duplicate_product_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-update-slug-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-update-slug-seller").await;
    let first = sample_product("postgres-product-update-slug-first", shop_id, seller_id);
    let second = sample_product("postgres-product-update-slug-second", shop_id, seller_id);
    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;
    insert_product_with_event(&unit_of_work, &product_listings, &events, &second).await;
    let conflict = rehydrate_product_for_update(
        &second,
        first.slug_id().clone(),
        second.shop_id(),
        second.shop_listing_id().clone(),
    );

    let result = {
        let mut tx = begin(&unit_of_work).await;
        let expected_event_id = match product_listings
            .in_transaction(&mut tx)
            .find_by_id(second.id())
            .await
        {
            Ok(Some(loaded)) => loaded.version,
            Ok(None) => panic!("missing second listing"),
            Err(error) => panic!("failed to find second listing: {error:?}"),
        };
        product_listings
            .in_transaction(&mut tx)
            .update(&conflict, expected_event_id, EventId::new())
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ProductListingSlugAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_roll_back_product_and_event_when_transaction_is_not_committed() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let shop_id = seed_shop(&pool, "product-listing-postgres-rollback-shop").await;
    let seller_id = seed_shop(&pool, "product-listing-postgres-rollback-seller").await;
    let product = sample_product("postgres-product-rollback", shop_id, seller_id);
    let event = first_stamped_event(&product);

    {
        let mut tx = begin(&unit_of_work).await;
        match product_listings
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
    let product_after_rollback = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find rolled-back product: {error:?}"),
    };

    let persisted_event_count: i64 = match sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product.id()))
    .fetch_one(&pool)
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to count rolled-back events: {error}"),
    };
    commit(tx).await;

    assert!(product_after_rollback.is_none());

    assert_eq!(0, persisted_event_count);
}

async fn insert_product_row(
    pool: &sqlx::PgPool,
    shop_id: ShopId,
    slug: &str,
    sale_observation_fx_rate_id: Option<FxRateId>,
    sale_observed_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    let product_listing_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_slug_id, event_id, content_source_event_id, shop_id, seller_id, shop_listing_id, lifecycle, url, sale_observation_fx_rate_id, sale_observed_at) VALUES ($1, $2, $3, $3, $4, $4, $5, 'ACTIVE', 'https://example.test/product', $6, $7)",
    )
    .bind(product_listing_id)
    .bind(slug)
    .bind(event_id)
    .bind(uuid::Uuid::from(shop_id))
    .bind(format!("{slug}-sku"))
    .bind(sale_observation_fx_rate_id.map(uuid::Uuid::from))
    .bind(sale_observed_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CREATED', 'DOMAIN', '{}', now())",
    )
    .bind(event_id)
    .bind(product_listing_id)
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
    product_listings: &SqlxProductListingRepositoryFactory,
    events: &SqlxProductListingEventStoreFactory,
    product: &ProductListing,
) {
    let event = first_stamped_event(product);
    let mut tx = begin(unit_of_work).await;
    match product_listings
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

fn first_stamped_event(
    product: &ProductListing,
) -> product_listing_service::ports::product_listing_event_store::ProductListingEvent {
    match stamp_product_listing_events(
        product.id(),
        OffsetDateTime::now_utc(),
        product.pending_event_payloads().to_vec(),
    )
    .into_iter()
    .next()
    {
        Some(event) => event,
        None => panic!("product is missing a pending event payload"),
    }
}

fn rehydrate_product_for_update(
    product: &ProductListing,
    slug_id: ProductListingSlugId,
    shop_id: ShopId,
    shop_listing_id: product_listing_core::shop_listing_id::ShopListingId,
) -> ProductListing {
    match ProductListing::rehydrate(RehydratedProductListingState {
        id: product.id(),
        slug_id,
        shop_id,
        seller_id: product.seller_id(),
        shop_listing_id,
        address: product.address(),
        title: product.title().cloned(),
        description: product.description().cloned(),
        pricing: product.pricing(),
        sale_observation: product.sale_observation(),
        availability: product.availability(),
        lifecycle: product.lifecycle(),
        url: product.url().clone(),
        images: product.images().clone(),
        auction: product.auction(),
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to rehydrate conflict product: {error:?}"),
    }
}

fn sample_product(slug: &str, shop_id: ShopId, seller_id: ShopId) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(&format!(
        "https://example.com/{slug}.jpg"
    ))));
    match ProductListing::create(NewProductListing {
        id: product_listing_core::product_listing_id::ProductListingId::new(),
        shop_id,
        seller_id,
        shop_listing_id: product_listing_core::shop_listing_id::ShopListingId::from(slug),
        address: ProductListingAddress::default(),
        title: Some(Localized::new(Language::En, Title::from(slug))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Nice product"),
        )),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: None,
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductListingAuction::default(),
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
