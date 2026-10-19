use application::transaction::{Transaction, UnitOfWork};
use geo_postgres::SqlxResolutionRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPublicCatalogueOverviewReaderFactory, geography::SqlxGeographyProjectionStoreFactory,
};
use product_listing_service::{
    geography::{ProjectListingGeographyHandler, ProjectListingGeographyUseCase},
    ports::{PublicCatalogueOverviewReader, PublicCatalogueOverviewReaderFactory},
    use_cases::queries::get_public_catalogue_overview::PublicCatalogueOverview,
};
use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct Listing {
    lifecycle: &'static str,
    availability: Option<&'static str>,
    title_language: Option<&'static str>,
    description_language: Option<&'static str>,
    created_hours_ago: i32,
}

impl Listing {
    const fn active() -> Self {
        Self {
            lifecycle: "ACTIVE",
            availability: Some("AVAILABLE"),
            title_language: Some("de"),
            description_language: None,
            created_hours_ago: 24 * 30,
        }
    }

    const fn withdrawn() -> Self {
        Self {
            lifecycle: "WITHDRAWN",
            availability: None,
            ..Self::active()
        }
    }
}

async fn read_overview() -> PublicCatalogueOverview {
    let pool = get_postgres_client().await;
    let mut tx = SqlxUnitOfWork::new(pool)
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin overview transaction: {error}"));
    let overview = SqlxPublicCatalogueOverviewReaderFactory::new()
        .in_transaction(&mut tx)
        .read_overview()
        .await
        .unwrap_or_else(|error| panic!("failed to read public overview: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit overview transaction: {error}"));
    overview
}

async fn database_now(pool: &sqlx::PgPool) -> OffsetDateTime {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read database clock: {error}"))
}

async fn seed_party(pool: &sqlx::PgPool) -> Uuid {
    let party_id = Uuid::now_v7();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("party-{}", party_id.simple()))
        .bind("Public overview party")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed party: {error}"));
    party_id
}

async fn seed_source(pool: &sqlx::PgPool, party_id: Uuid) -> Uuid {
    let listing_source_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(listing_source_id)
    .bind(format!("source-{}", listing_source_id.simple()))
    .bind("Public overview source")
    .bind(party_id)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    listing_source_id
}

async fn seed_listing(pool: &sqlx::PgPool, listing_source_id: Uuid, listing: Listing) -> Uuid {
    let product_listing_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();
    let slug = format!(
        "overview-{}",
        &product_listing_id.simple().to_string()[26..]
    );
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin listing seed: {error}"));
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, lifecycle, availability, url, created) VALUES ($1, $2, $3, $3, $3, $4, $5, CASE WHEN $6::text IS NULL THEN NULL ELSE 'Title' END, $6, CASE WHEN $7::text IS NULL THEN NULL ELSE 'Description' END, $7, $8, $9, 'https://example.test/listing', now() - $10::int * interval '1 hour')",
    )
    .bind(product_listing_id)
    .bind(slug)
    .bind(event_id)
    .bind(listing_source_id)
    .bind(product_listing_id.to_string())
    .bind(listing.title_language)
    .bind(listing.description_language)
    .bind(listing.lifecycle)
    .bind(listing.availability)
    .bind(listing.created_hours_ago)
    .execute(&mut *tx)
    .await
    .unwrap_or_else(|error| panic!("failed to seed product listing: {error}"));
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())",
    )
    .bind(event_id)
    .bind(product_listing_id)
    .execute(&mut *tx)
    .await
    .unwrap_or_else(|error| panic!("failed to seed product listing event: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit listing seed: {error}"));
    product_listing_id
}

fn dimension(country: Option<&str>) -> serde_json::Value {
    json!({
        "location_id": Uuid::now_v7(),
        "basis": "SOURCE_DEFAULT_DERIVED",
        "reference_release": "iso-codes-4.20.1",
        "country": country,
        "subdivision": null,
        "position": country.map_or_else(
            || json!({ "point": { "latitude_degrees": 48.1, "longitude_degrees": 11.5 }, "precision": "PREMISES", "accuracy_metres": null }),
            |_| serde_json::Value::Null,
        ),
    })
}

async fn seed_geography(
    pool: &sqlx::PgPool,
    product_listing_id: Uuid,
    business: Option<&str>,
    inventory: Option<Option<&str>>,
) {
    sqlx::query(
        "INSERT INTO product_listing_geographies (product_listing_id, dependency, geography, geography_version) VALUES ($1, '{}', $2, 1)",
    )
    .bind(product_listing_id)
    .bind(json!({
        "business": business.map(|country| dimension(Some(country))),
        "inventory": inventory.map(dimension),
    }))
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing geography: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_zero_counts_at_calculation_time_for_empty_catalogue() {
    let pool = get_postgres_client().await;
    let before = database_now(&pool).await;

    let overview = read_overview().await;

    let after = database_now(&pool).await;
    assert!(before <= overview.as_of && overview.as_of <= after);
    assert_eq!(time::UtcOffset::UTC, overview.as_of.offset());
    assert_eq!(
        PublicCatalogueOverview {
            as_of: overview.as_of,
            sources_indexed: 0,
            live_objects: 0,
            countries: 0,
            languages: 0,
            new_objects_last_7_days: 0,
        },
        overview
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_only_active_listings_and_sources_with_live_listings() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let live_source = seed_source(&pool, party_id).await;
    let withdrawn_only_source = seed_source(&pool, party_id).await;
    let _empty_source = seed_source(&pool, party_id).await;
    for availability in [Some("AVAILABLE"), Some("SOLD_OUT"), None] {
        seed_listing(
            &pool,
            live_source,
            Listing {
                availability,
                ..Listing::active()
            },
        )
        .await;
    }
    seed_listing(&pool, live_source, Listing::withdrawn()).await;
    seed_listing(&pool, withdrawn_only_source, Listing::withdrawn()).await;

    let overview = read_overview().await;

    assert_eq!(3, overview.live_objects);
    assert_eq!(1, overview.sources_indexed);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_union_original_title_and_description_languages_without_multiplying_counts() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let source = seed_source(&pool, party_id).await;
    let mut listings = Vec::new();
    for (title_language, description_language) in [
        (Some("de"), Some("en")),
        (Some("en"), Some("en")),
        (Some("en"), None),
        (None, Some("fr")),
        (None, None),
    ] {
        listings.push(
            seed_listing(
                &pool,
                source,
                Listing {
                    title_language,
                    description_language,
                    ..Listing::active()
                },
            )
            .await,
        );
    }
    seed_listing(
        &pool,
        source,
        Listing {
            title_language: Some("it"),
            description_language: Some("ja"),
            ..Listing::withdrawn()
        },
    )
    .await;
    sqlx::query(
        "INSERT INTO product_listing_translations (product_listing_id, source_event_id, language, title) SELECT product_listing_id, current_event_id, 'pl', 'Generated title' FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(listings[0])
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed generated translation: {error}"));

    let overview = read_overview().await;

    assert_eq!(3, overview.languages);
    assert_eq!(5, overview.live_objects);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_distinct_advertised_inventory_countries_of_live_listings() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let source = seed_source(&pool, party_id).await;
    for (business, inventory) in [
        (None, Some(Some("DE"))),
        (Some("AT"), Some(Some("DE"))),
        (None, Some(Some("FR"))),
        (Some("CH"), None),
        (None, Some(None)),
    ] {
        let listing = seed_listing(&pool, source, Listing::active()).await;
        seed_geography(&pool, listing, business, inventory).await;
    }
    seed_listing(&pool, source, Listing::active()).await;
    let withdrawn = seed_listing(&pool, source, Listing::withdrawn()).await;
    seed_geography(&pool, withdrawn, None, Some(Some("IT"))).await;

    let overview = read_overview().await;

    assert_eq!(2, overview.countries);
    assert_eq!(6, overview.live_objects);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_live_listings_created_in_the_seven_days_ending_at_as_of() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let source = seed_source(&pool, party_id).await;
    for created_hours_ago in [0, 24 * 6, 24 * 7 - 1, 24 * 7 + 1, 24 * 30, -1] {
        seed_listing(
            &pool,
            source,
            Listing {
                created_hours_ago,
                ..Listing::active()
            },
        )
        .await;
    }
    seed_listing(
        &pool,
        source,
        Listing {
            created_hours_ago: 1,
            ..Listing::withdrawn()
        },
    )
    .await;

    let overview = read_overview().await;

    assert_eq!(3, overview.new_objects_last_7_days);
    assert_eq!(6, overview.live_objects);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_inventory_country_from_the_effective_geography_projection() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let inventory_source = seed_source(&pool, party_id).await;
    let business_source = seed_source(&pool, party_id).await;
    for (source, country, scope) in [
        (inventory_source, "DE", "ADVERTISED_INVENTORY"),
        (business_source, "FR", "ASSOCIATED_BUSINESS"),
    ] {
        let location_id = Uuid::now_v7();
        sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,position,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Site',$3,NULL,'ACTIVE','SERVICE:test','SERVICE:test')")
            .bind(location_id)
            .bind(party_id)
            .bind(json!({"reference_release":"iso-codes-4.20.1","address_text":"Supplied address","country":country,"subdivision":null,"postal_code":null}))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed party location: {error}"));
        let evidence = json!({ "scope": scope, "reference": "source catalogue" });
        let (business, inventory) = if scope == "ADVERTISED_INVENTORY" {
            (None, Some(evidence))
        } else {
            (Some(evidence), None)
        };
        sqlx::query("INSERT INTO listing_source_location_defaults(listing_source_id,party_id,party_location_id,business_evidence,inventory_evidence,revision,updated_actor) VALUES($1,$2,$3,$4,$5,2,'SERVICE:test')")
            .bind(source)
            .bind(party_id)
            .bind(location_id)
            .bind(business)
            .bind(inventory)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed source location default: {error}"));
        seed_listing(&pool, source, Listing::active()).await;
        sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)")
            .bind(source.to_string())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to enqueue listing geography: {error}"));
    }
    let handler = ProjectListingGeographyHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxGeographyProjectionStoreFactory,
        SqlxResolutionRepositoryFactory,
    );
    for _ in 0..20 {
        handler
            .execute(None, 100)
            .await
            .unwrap_or_else(|error| panic!("failed to project listing geography: {error}"));
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_geography_work WHERE pending)")
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("failed to read geography work: {error}"));
        if !pending {
            break;
        }
    }
    let projected: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_geographies")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count projections: {error}"));
    assert_eq!(2, projected);

    let overview = read_overview().await;

    assert_eq!(1, overview.countries);
}
