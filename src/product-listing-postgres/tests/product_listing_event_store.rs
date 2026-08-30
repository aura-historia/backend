use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ListingSaleObservation, ListingSaleObservationRetracted, ListingSaleObserved,
    NewProductListing, ProductListing, ProductListingAuction, ProductListingAuctionChanged,
    ProductListingEventPayload, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use listing_source_core::ListingSourceId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingEventReaderFactory,
    SqlxProductListingEventStoreFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventReader, ProductListingEventReaderFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryFactory, stamp_product_listing_events,
};
use product_listing_service::use_cases::{
    ProductListingEventLookup, UpsertProductListingCommand, UpsertProductListingHandler,
    UpsertProductListingResult, UpsertProductListingUseCase,
};
use sqlx::AssertSqlSafe;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime, UtcOffset};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_round_trip_timestamp_payloads_through_product_listing_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let event_store = SqlxProductListingEventStoreFactory::new();
    let event_reader = SqlxProductListingEventReaderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-event-timestamps-source").await;
    let source_offset =
        UtcOffset::from_hms(5, 30, 0).unwrap_or_else(|error| panic!("source offset: {error}"));
    let auction_start = (OffsetDateTime::from_unix_timestamp(1_700_000_000)
        .unwrap_or_else(|error| panic!("auction start: {error}"))
        + Duration::nanoseconds(123_456_789))
    .to_offset(source_offset);
    let auction_end = (auction_start + Duration::hours(3)).to_offset(source_offset);
    let observation = ListingSaleObservation::new(
        (auction_end + Duration::nanoseconds(987_654_321)).to_offset(source_offset),
        fxrate_core::FxRateId::new(),
    );
    let product = sample_product_with_auction(
        "postgres-product-event-timestamps",
        listing_source_id,
        ProductListingAuction {
            start: Some(auction_start),
            end: Some(auction_end),
        },
    );
    let created = first_stamped_event(&product);
    let expected_payloads = vec![
        created.payload.clone(),
        ProductListingEventPayload::AuctionChanged(ProductListingAuctionChanged {
            auction: ProductListingAuction {
                start: Some((auction_start + Duration::days(1)).to_offset(source_offset)),
                end: Some((auction_end + Duration::days(1)).to_offset(source_offset)),
            },
        }),
        ProductListingEventPayload::SaleObserved(ListingSaleObserved { observation }),
        ProductListingEventPayload::SaleObservationRetracted(ListingSaleObservationRetracted {
            observation,
        }),
    ];
    let events = stamp_product_listing_events(
        product.id(),
        OffsetDateTime::now_utc(),
        expected_payloads.clone(),
    );
    let current_event_id = events
        .first()
        .map(|event| event.event_id)
        .unwrap_or_else(|| panic!("created event is missing"));

    let mut tx = begin(&unit_of_work).await;
    product_listings
        .in_transaction(&mut tx)
        .insert(&product, current_event_id)
        .await
        .unwrap_or_else(|error| panic!("insert listing: {error:?}"));
    for event in &events {
        event_store
            .in_transaction(&mut tx)
            .append(event)
            .await
            .unwrap_or_else(|error| panic!("append event: {error:?}"));
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let reconstructed = event_reader
        .in_transaction(&mut tx)
        .find_domain_events(&ProductListingEventLookup::ById(product.id()))
        .await
        .unwrap_or_else(|error| panic!("read events: {error:?}"))
        .unwrap_or_else(|| panic!("listing history is missing"));
    commit(tx).await;

    assert_eq!(expected_payloads.len(), reconstructed.len());
    for expected in expected_payloads {
        assert!(
            reconstructed
                .iter()
                .any(|actual| actual.payload == expected)
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retry_same_key_upserts_after_a_real_postgres_insert_race() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-upsert-race-source").await;
    install_same_key_insert_barrier(&pool, listing_source_id).await;

    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let handler = UpsertProductListingHandler::new(
        unit_of_work,
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventStoreFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let context = system_context();
    let first_command = upsert_command(
        listing_source_id,
        "postgres-product-upsert-race",
        "Concurrent listing first",
        1_200,
    );
    let second_command = upsert_command(
        listing_source_id,
        "postgres-product-upsert-race",
        "Concurrent listing second",
        1_300,
    );
    let (first, second) = tokio::join!(
        handler.execute(&context, first_command),
        handler.execute(&context, second_command),
    );

    remove_same_key_insert_barrier(&pool).await;

    let (first, second) = match (first, second) {
        (Ok(first), Ok(second)) => (first, second),
        (first, second) => panic!("upsert results: {first:?}, {second:?}"),
    };
    let (product_listing_id, expected_price_amount) = match (&first, &second) {
        (
            UpsertProductListingResult::Created(created),
            UpsertProductListingResult::Updated(updated),
        ) => {
            assert_eq!(created.product_listing_id, updated.product_listing_id);
            (created.product_listing_id, 1_300_i64)
        }
        (
            UpsertProductListingResult::Updated(updated),
            UpsertProductListingResult::Created(created),
        ) => {
            assert_eq!(created.product_listing_id, updated.product_listing_id);
            (created.product_listing_id, 1_200_i64)
        }
        _ => panic!("expected one create and one retry update: {first:?}, {second:?}"),
    };

    let listing_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind("postgres-product-upsert-race")
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count listings: {error}"));
    assert_eq!(1, listing_count);

    let price_amount: i64 = sqlx::query_scalar(
        "SELECT price_amount FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read persisted price: {error}"));
    assert_eq!(expected_price_amount, price_amount);

    let orphan_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events e LEFT JOIN product_listings p ON p.product_listing_id = e.product_listing_id WHERE e.product_listing_id = $1 AND p.product_listing_id IS NULL",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count orphan events: {error}"));
    assert_eq!(0, orphan_count);

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count listing events: {error}"));
    assert_eq!(2, event_count);

    let current_event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listings p JOIN product_listing_events e ON e.event_id = p.event_id AND e.product_listing_id = p.product_listing_id WHERE p.product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("verify current event: {error}"));
    assert_eq!(1, current_event_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_event_in_product_listing_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-conflict-source").await;
    let product = sample_product("postgres-product-conflict", listing_source_id);
    let event = first_stamped_event(&product);

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append first event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let duplicate_event = events.in_transaction(&mut tx).append(&event).await;
    assert!(matches!(
        duplicate_event,
        Err(ProductListingEventStoreError::ProductListingEventAlreadyExists)
    ));
}

fn upsert_command(
    listing_source_id: ListingSourceId,
    source_listing_id: &str,
    title: &str,
    price_amount: u64,
) -> UpsertProductListingCommand {
    UpsertProductListingCommand {
        listing_source_id,
        source_listing_id: SourceListingId::try_from(source_listing_id)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Some(Localized::new(Language::En, Title::from(title))),
        description: None,
        price: application::patch_field::PatchField::Set(Price::new(
            MonetaryAmount::from(price_amount),
            Currency::Eur,
        )),
        price_estimate_min: application::patch_field::PatchField::Unchanged,
        price_estimate_max: application::patch_field::PatchField::Unchanged,
        availability: application::patch_field::PatchField::Set(ListingAvailability::InStock),
        url: Some(url("https://example.com/concurrent-listing")),
        images: application::patch_field::PatchField::Unchanged,
        auction_start: application::patch_field::PatchField::Unchanged,
        auction_end: application::patch_field::PatchField::Unchanged,
    }
}

fn system_context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("product-listing-upsert-race"),
        correlation_id: CorrelationId::new("product-listing-upsert-race"),
    }
}

async fn install_same_key_insert_barrier(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    let sql = format!(
        r#"
        CREATE OR REPLACE FUNCTION product_listing_same_key_insert_barrier()
        RETURNS trigger LANGUAGE plpgsql AS $$
        DECLARE attempts integer := 0;
        BEGIN
            IF NEW.listing_source_id = '{listing_source_id}'::uuid THEN
                PERFORM pg_advisory_lock_shared(87654321, 42);
                LOOP
                    EXIT WHEN (
                        SELECT count(*)
                        FROM pg_locks
                        WHERE locktype = 'advisory'
                          AND mode = 'ShareLock'
                          AND classid = 87654321
                          AND objid = 42
                          AND granted
                    ) >= 2;
                    attempts := attempts + 1;
                    IF attempts > 500 THEN
                        PERFORM pg_advisory_unlock_shared(87654321, 42);
                        RAISE EXCEPTION 'same-key insert barrier timed out';
                    END IF;
                    PERFORM pg_sleep(0.01);
                END LOOP;
                PERFORM pg_sleep(0.1);
                PERFORM pg_advisory_unlock_shared(87654321, 42);
            END IF;
            RETURN NEW;
        END;
        $$;
        "#,
    );
    sqlx::query(AssertSqlSafe(sql))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("install insert barrier function: {error}"));
    sqlx::query(
        "CREATE TRIGGER product_listing_same_key_insert_barrier_trigger BEFORE INSERT ON product_listings FOR EACH ROW EXECUTE FUNCTION product_listing_same_key_insert_barrier()",
    )
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("install insert barrier trigger: {error}"));
}

async fn remove_same_key_insert_barrier(pool: &sqlx::PgPool) {
    sqlx::query(
        "DROP TRIGGER IF EXISTS product_listing_same_key_insert_barrier_trigger ON product_listings",
    )
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("remove insert barrier trigger: {error}"));
    sqlx::query("DROP FUNCTION IF EXISTS product_listing_same_key_insert_barrier()")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("remove insert barrier function: {error}"));
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

fn sample_product(slug: &str, listing_source_id: ListingSourceId) -> ProductListing {
    sample_product_with_auction(slug, listing_source_id, ProductListingAuction::default())
}

fn sample_product_with_auction(
    slug: &str,
    listing_source_id: ListingSourceId,
    auction: ProductListingAuction,
) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(&format!(
        "https://example.com/{slug}.jpg"
    ))));
    match ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::from_title_and_suffix(slug, "a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        listing_source_id,
        source_listing_id: SourceListingId::try_from(slug)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
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
        availability: Some(ListingAvailability::Available),
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction,
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing-source party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    listing_source_id
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
