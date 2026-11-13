//! Operator presentation joined by the ProductListing readers that do not have their own suite.
//! The active/dissolved/absent Partnership semantics are proven once in
//! `product_listing_details_reader.rs`; these tests prove each statement's wiring.

use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    listing_availability::ListingAvailability,
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_postgres::{
    SqlxListingSourceSummaryReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory, SqlxProductListingSearchFilterMatchSourceReaderFactory,
    SqlxProductListingWatchlistNotificationSourceReaderFactory,
};
use product_listing_service::ports::{
    ListingSourceOperatorSummary, ListingSourceSummaryReader, ProductListingEventAppender,
    ProductListingEventAppenderFactory, ProductListingRepository, ProductListingRepositoryFactory,
    ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory,
    ProductListingWatchlistNotificationSourceReadOutcome,
    ProductListingWatchlistNotificationSourceReader,
    ProductListingWatchlistNotificationSourceReaderFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_hydrate_search_source_summaries_with_operator_partner_status() {
    let pool = get_postgres_client().await;
    let partner = seed_listing_source(&pool, "summary-partner").await;
    let other = seed_listing_source(&pool, "summary-other").await;
    insert_active_partnership(&pool, partner).await;

    let summaries = SqlxListingSourceSummaryReader::new(pool.clone())
        .find_summaries(&[partner, other])
        .await
        .unwrap_or_else(|error| panic!("read source summaries: {error:?}"));

    let operator = |id: ListingSourceId| {
        summaries
            .get(&id)
            .map(|summary| summary.summary.operator.clone())
            .unwrap_or_else(|| panic!("missing source summary"))
    };
    assert_eq!(
        expected_operator("summary-partner", true),
        operator(partner)
    );
    assert_eq!(expected_operator("summary-other", false), operator(other));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_hydrate_match_and_notification_sources_with_operator_partner_status() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let listing_source_id = seed_listing_source(&pool, "event-sources").await;
    insert_active_partnership(&pool, listing_source_id).await;
    let (product_listing_id, changed_event_id) =
        persist_withdrawn_product(&unit_of_work, listing_source_id).await;

    let mut transaction = begin(&unit_of_work).await;
    let match_source = SqlxProductListingSearchFilterMatchSourceReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_source(changed_event_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("read match source: {error:?}"))
        .unwrap_or_else(|| panic!("match source is missing"));
    let notification_source = SqlxProductListingWatchlistNotificationSourceReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_source(changed_event_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("read notification source: {error:?}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit reads: {error}"));

    let ProductListingWatchlistNotificationSourceReadOutcome::Found(notification_source) =
        notification_source
    else {
        panic!("notification source is missing");
    };
    let expected = expected_operator("event-sources", true);
    assert_eq!(expected, match_source.source.operator);
    assert_eq!(expected, notification_source.source.operator);
}

fn expected_operator(slug: &str, partner: bool) -> ListingSourceOperatorSummary {
    ListingSourceOperatorSummary {
        name: party_core::party_name::PartyName::try_from(format!("{slug} party"))
            .unwrap_or_else(|error| panic!("valid operator name: {error}")),
        partner,
    }
}

/// Persists a discovered listing and its withdrawal, whose availability change is a
/// notification-relevant event.
async fn persist_withdrawn_product(
    unit_of_work: &SqlxUnitOfWork,
    listing_source_id: ListingSourceId,
) -> (ProductListingId, domain_primitives::event_id::EventId) {
    let mut product = sample_product(listing_source_id);
    let discovered = stamp_product_listing_event(
        product.id(),
        OffsetDateTime::now_utc(),
        product
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("new product is missing discovered event")),
    );
    let mut transaction = begin(unit_of_work).await;
    let created = SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(&product, discovered.event_id)
        .await
        .unwrap_or_else(|error| panic!("insert product: {error:?}"));
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&discovered)
        .await
        .unwrap_or_else(|error| panic!("append discovered event: {error:?}"));

    product
        .withdraw()
        .unwrap_or_else(|error| panic!("withdraw product: {error}"));
    let withdrawn = stamp_product_listing_event(
        product.id(),
        OffsetDateTime::now_utc(),
        product
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("withdrawn product event is missing")),
    );
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .update(
            &product,
            created.version,
            withdrawn.event_id,
            ProductListingWriteEffects::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("update product: {error:?}"));
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&withdrawn)
        .await
        .unwrap_or_else(|error| panic!("append withdrawn event: {error:?}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit product: {error}"));
    (product.id(), withdrawn.event_id)
}

fn sample_product(listing_source_id: ListingSourceId) -> ProductListing {
    ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::raw("operator-sources-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product slug: {error}")),
        listing_source_id,
        source_listing_id: SourceListingId::try_from("operator-sources-listing")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Some(Localized::new(
            Language::En,
            Title::from("Operator sources"),
        )),
        description: None,
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(12_u64), Currency::Eur).into()),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: Some(ListingAvailability::Available),
        url: url("https://example.com/operator-sources"),
        images: IndexSet::from([ProductListingImage::new(url(
            "https://example.com/operator-sources.jpg",
        ))]),
        auction: None,
    })
    .unwrap_or_else(|error| panic!("create product: {error}"))
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}

async fn insert_active_partnership(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO partnerships (partnership_id, party_id, business_state) \
         SELECT $1, operator_party_id, 'ACTIVE' FROM listing_sources WHERE listing_source_id = $2",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(listing_source_id.into_uuid())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed operator partnership: {error}"));
}

fn url(value: &str) -> Url {
    Url::parse(value).unwrap_or_else(|error| panic!("valid URL: {error}"))
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"))
}
