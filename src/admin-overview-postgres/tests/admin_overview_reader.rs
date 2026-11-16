use admin_overview_postgres::SqlxAdminOverviewReaderFactory;
use admin_overview_service::{
    ports::{AdminOverviewReader, AdminOverviewReaderFactory},
    use_cases::get_admin_overview::AdminOverview,
};
use application::transaction::{Transaction, UnitOfWork};
use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn read_overview() -> AdminOverview {
    let pool = get_postgres_client().await;
    let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
    let mut tx = match unit_of_work.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin overview transaction: {error}"),
    };
    let result = SqlxAdminOverviewReaderFactory::new()
        .in_transaction(&mut tx)
        .read_overview()
        .await;
    match result {
        Ok(overview) => match tx.commit().await {
            Ok(()) => overview,
            Err(error) => panic!("failed to commit overview transaction: {error}"),
        },
        Err(error) => panic!("failed to read overview: {error}"),
    }
}

async fn seed_user(pool: &sqlx::PgPool, tier: &str, role: &str) -> Uuid {
    let user_id = Uuid::now_v7();
    let result =
        sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, $4)")
            .bind(user_id)
            .bind(format!("{user_id}@admin-overview.test"))
            .bind(tier)
            .bind(role)
            .execute(pool)
            .await;
    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }
    user_id
}

async fn seed_party(pool: &sqlx::PgPool) -> Uuid {
    let party_id = Uuid::now_v7();
    let result =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id)
            .bind(format!("party-{}", Uuid::new_v4().simple()))
            .bind("Admin overview party")
            .execute(pool)
            .await;
    if let Err(error) = result {
        panic!("failed to seed party: {error}");
    }
    party_id
}

async fn seed_listing_source(pool: &sqlx::PgPool, party_id: Uuid) -> Uuid {
    let listing_source_id = Uuid::now_v7();
    let result = sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')",
        )
        .bind(listing_source_id)
        .bind(format!("source-{}", Uuid::new_v4().simple()))
        .bind("Admin overview source")
        .bind(party_id)
        .execute(pool)
        .await;
    if let Err(error) = result {
        panic!("failed to seed listing source: {error}");
    }
    listing_source_id
}

async fn add_ingestion_method(pool: &sqlx::PgPool, listing_source_id: Uuid, method: &str) {
    let result = sqlx::query(
            "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, $2)",
        )
        .bind(listing_source_id)
        .bind(method)
        .execute(pool)
        .await;
    if let Err(error) = result {
        panic!("failed to seed ingestion method: {error}");
    }
}

async fn seed_partnership(pool: &sqlx::PgPool, party_id: Uuid) -> Uuid {
    let partnership_id = Uuid::now_v7();
    let result = sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(party_id)
        .execute(pool)
        .await;
    if let Err(error) = result {
        panic!("failed to seed partnership: {error}");
    }
    partnership_id
}

async fn seed_application(
    pool: &sqlx::PgPool,
    applicant_user_id: Uuid,
    state: &str,
    listing_source_id: Uuid,
    approved_partnership_id: Option<Uuid>,
) {
    let result = sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(applicant_user_id)
        .bind(state)
        .bind(json!({ "type": "EXISTING_LISTING_SOURCE", "listing_source_id": listing_source_id.to_string() }))
        .bind(approved_partnership_id)
        .bind(approved_partnership_id.map(|_| listing_source_id))
        .execute(pool)
        .await;
    if let Err(error) = result {
        panic!("failed to seed partnership application: {error}");
    }
}

async fn seed_product_listing(
    pool: &sqlx::PgPool,
    listing_source_id: Uuid,
    lifecycle: &str,
    availability: Option<&str>,
) {
    let product_listing_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();
    let slug_suffix = Uuid::new_v4().simple().to_string();
    let product_listing_slug_id = format!("listing-{}", &slug_suffix[..6]);
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin product listing seed transaction: {error}"),
    };
    let listing_result = sqlx::query(
            "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, lifecycle, availability, url) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8)",
        )
        .bind(product_listing_id)
        .bind(product_listing_slug_id)
        .bind(event_id)
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .bind(lifecycle)
        .bind(availability)
        .bind("https://example.test/listing")
        .execute(&mut *transaction)
        .await;
    if let Err(error) = listing_result {
        panic!("failed to seed product listing: {error}");
    }
    let event_result = sqlx::query(
            "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
        )
        .bind(event_id)
        .bind(product_listing_id)
        .bind(json!({}))
        .execute(&mut *transaction)
        .await;
    if let Err(error) = event_result {
        panic!("failed to seed product listing event: {error}");
    }
    if let Err(error) = transaction.commit().await {
        panic!("failed to commit product listing seed transaction: {error}");
    }
}

async fn seed_content_notice(pool: &sqlx::PgPool, listing_source_id: Uuid) {
    let content_notice_id = Uuid::now_v7();
    let result = sqlx::query(
            "INSERT INTO content_notices (content_notice_id, target_type, listing_source_id, listing_source_name, listing_source_partner, submitter_role, reason_category, submitter_name, submitter_email, submitter_language, good_faith_statement_version, submission_fingerprint, business_state) VALUES ($1, 'LISTING_SOURCE', $2, 'Admin overview source', false, 'PROVIDER', 'PROVIDER_OPT_OUT', 'Notifier', $3, 'en', 'GOOD_FAITH_V1', $4, 'SUBMITTED')",
        )
        .bind(content_notice_id)
        .bind(listing_source_id)
        .bind(format!("{content_notice_id}@admin-overview.test"))
        .bind(content_notice_id.as_bytes().repeat(2))
        .execute(pool)
        .await;
    if let Err(error) = result {
        panic!("failed to seed content notice: {error}");
    }
}

async fn execute(pool: &sqlx::PgPool, statement: &'static str) {
    if let Err(error) = sqlx::query(statement).execute(pool).await {
        panic!("failed to execute `{statement}`: {error}");
    }
}

/// Seeds listings numbered `first..=last` with one discovery event each.
async fn seed_product_listings_in_bulk(
    pool: &sqlx::PgPool,
    listing_source_id: Uuid,
    (first, last): (i64, i64),
    lifecycle: &str,
    availability: Option<&str>,
) {
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin bulk product listing seed transaction: {error}"),
    };
    let listings_result = sqlx::query(
            "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, lifecycle, availability, url) SELECT md5('listing-' || n)::uuid, 'bulk-listing-' || n || '-' || substr(md5(n::text), 1, 6), md5('event-' || n)::uuid, md5('event-' || n)::uuid, md5('event-' || n)::uuid, $3, 'bulk-' || n, $4, $5, 'https://example.test/listing' FROM generate_series($1::bigint, $2::bigint) n",
        )
        .bind(first)
        .bind(last)
        .bind(listing_source_id)
        .bind(lifecycle)
        .bind(availability)
        .execute(&mut *transaction)
        .await;
    if let Err(error) = listings_result {
        panic!("failed to bulk seed product listings: {error}");
    }
    let events_result = sqlx::query(
            "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) SELECT md5('event-' || n)::uuid, md5('listing-' || n)::uuid, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}'::jsonb, now() FROM generate_series($1::bigint, $2::bigint) n",
        )
        .bind(first)
        .bind(last)
        .execute(&mut *transaction)
        .await;
    if let Err(error) = events_result {
        panic!("failed to bulk seed product listing events: {error}");
    }
    if let Err(error) = transaction.commit().await {
        panic!("failed to commit bulk product listing seed transaction: {error}");
    }
}

/// Committed inserts reach the cumulative statistics asynchronously; ANALYZE must run after
/// them, or their late report would be added on top of the live count it measured.
async fn wait_for_reported_inserts(pool: &sqlx::PgPool, expected: i64) {
    for _ in 0..100 {
        let reported = sqlx::query_scalar::<_, i64>(
            "SELECT n_tup_ins FROM pg_stat_user_tables WHERE relid = 'product_listings'::regclass",
        )
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read insert statistics: {error}"));
        if reported >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("product listing inserts were not reported to the cumulative statistics");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_empty_overview() {
    assert_eq!(AdminOverview::default(), read_overview().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_aggregate_representative_authoritative_data() {
    let pool = get_postgres_client().await;
    let free_user = seed_user(&pool, "FREE", "USER").await;
    let pro_admin = seed_user(&pool, "PRO", "ADMIN").await;
    let _ultimate_user = seed_user(&pool, "ULTIMATE", "USER").await;
    let first_party = seed_party(&pool).await;
    let second_party = seed_party(&pool).await;
    let first_source = seed_listing_source(&pool, first_party).await;
    let second_source = seed_listing_source(&pool, second_party).await;
    add_ingestion_method(&pool, first_source, "WEB_CRAWL").await;
    add_ingestion_method(&pool, first_source, "SHOPIFY").await;
    let partnership_id = seed_partnership(&pool, first_party).await;
    seed_application(&pool, free_user, "SUBMITTED", first_source, None).await;
    seed_application(
        &pool,
        pro_admin,
        "APPROVED",
        first_source,
        Some(partnership_id),
    )
    .await;
    seed_product_listing(&pool, first_source, "ACTIVE", Some("AVAILABLE")).await;
    seed_product_listing(&pool, first_source, "ACTIVE", None).await;
    seed_product_listing(&pool, second_source, "WITHDRAWN", None).await;

    let overview = read_overview().await;

    assert_eq!(3, overview.users.total);
    assert_eq!(1, overview.users.by_tier.free);
    assert_eq!(1, overview.users.by_tier.pro);
    assert_eq!(1, overview.users.by_tier.ultimate);
    assert_eq!(2, overview.users.by_role.user);
    assert_eq!(1, overview.users.by_role.admin);
    assert_eq!(2, overview.partnership_applications.total);
    assert_eq!(1, overview.partnership_applications.by_state.submitted);
    assert_eq!(1, overview.partnership_applications.by_state.approved);
    assert_eq!(2, overview.parties_total);
    assert_eq!(2, overview.listing_sources.total);
    assert_eq!(1, overview.listing_sources.without_ingestion_method);
    assert_eq!(1, overview.listing_sources.method_assignments.web_crawl);
    assert_eq!(1, overview.listing_sources.method_assignments.shopify);
    assert_eq!(0, overview.listing_sources.method_assignments.woocommerce);
    assert_eq!(0, overview.listing_sources.method_assignments.partner_api);
    assert_eq!(1, overview.partnerships_total);
    assert_eq!(3, overview.product_listings.total);
    assert_eq!(2, overview.product_listings.by_lifecycle.active);
    assert_eq!(1, overview.product_listings.by_lifecycle.withdrawn);
    assert_eq!(1, overview.product_listings.active_availability.available);
    assert_eq!(1, overview.product_listings.active_without_availability);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_count_submitted_content_notices() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let listing_source_id = seed_listing_source(&pool, party_id).await;
    seed_content_notice(&pool, listing_source_id).await;
    seed_content_notice(&pool, listing_source_id).await;
    seed_content_notice(&pool, Uuid::now_v7()).await;

    let overview = read_overview().await;

    assert_eq!(3, overview.content_notices.total);
    assert_eq!(3, overview.content_notices.by_state.submitted);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_estimate_product_listing_counts_from_planner_statistics_for_large_tables() {
    let pool = get_postgres_client().await;
    execute(
        &pool,
        "ALTER TABLE product_listings SET (autovacuum_enabled = false)",
    )
    .await;
    let party_id = seed_party(&pool).await;
    let source = seed_listing_source(&pool, party_id).await;
    seed_product_listings_in_bulk(&pool, source, (1, 60_000), "ACTIVE", Some("AVAILABLE")).await;
    seed_product_listings_in_bulk(&pool, source, (60_001, 80_000), "ACTIVE", Some("SOLD_OUT"))
        .await;
    seed_product_listings_in_bulk(&pool, source, (80_001, 90_000), "ACTIVE", None).await;
    seed_product_listings_in_bulk(&pool, source, (90_001, 100_000), "WITHDRAWN", None).await;
    wait_for_reported_inserts(&pool, 100_000).await;
    execute(&pool, "ANALYZE product_listings").await;
    // Sold after the analyze: an exact count would show 50,000 available and 30,000 sold out.
    execute(
        &pool,
        "UPDATE product_listings SET availability = 'SOLD_OUT' WHERE product_listing_id IN (SELECT product_listing_id FROM product_listings WHERE availability = 'AVAILABLE' LIMIT 10000)",
    )
    .await;

    let listings = read_overview().await.product_listings;

    let within = |expected: u64, actual: u64| actual.abs_diff(expected) <= 2_000;
    assert!(within(100_000, listings.total), "{listings:?}");
    assert!(within(90_000, listings.by_lifecycle.active), "{listings:?}");
    assert!(
        within(60_000, listings.active_availability.available),
        "{listings:?}"
    );
    assert!(
        within(20_000, listings.active_availability.sold_out),
        "{listings:?}"
    );
    assert!(
        within(10_000, listings.active_without_availability),
        "{listings:?}"
    );
    let availability = &listings.active_availability;
    assert_eq!(
        listings.total,
        listings.by_lifecycle.active + listings.by_lifecycle.withdrawn
    );
    assert_eq!(
        listings.by_lifecycle.active,
        availability.available
            + availability.in_stock
            + availability.limited_availability
            + availability.back_order
            + availability.made_to_order
            + availability.pre_order
            + availability.pre_sale
            + availability.unavailable
            + availability.reserved
            + availability.out_of_stock
            + availability.sold_out
            + listings.active_without_availability
    );
}
