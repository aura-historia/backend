use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use geo::CountryCode;
use listing_source_core::{ListingSourceId, ListingSourceSearch, SortListingSourceField};
use listing_source_postgres::SqlxListingSourceSearchReaderFactory;
use listing_source_service::ports::{ListingSourceSearchReader, ListingSourceSearchReaderFactory};
use listing_source_service::use_cases::queries::search_listing_sources::{
    SearchListingSourcesRequest, SearchListingSourcesResult,
};
use platform_postgres::SqlxUnitOfWork;
use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use time::macros::datetime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_continue_after_the_anchor_is_deleted_or_leaves_the_filter() {
    let pool = get_postgres_client().await;
    let operator = insert_operator(&pool).await;
    let ids = [1, 2, 3, 4, 5].map(ordered_source_id);
    for (id, name) in ids
        .iter()
        .zip(["Anchor A", "Anchor B", "Anchor C", "Anchor D", "Anchor E"])
    {
        insert_source(&pool, *id, name, operator, datetime!(2026-01-01 00:00 UTC)).await;
    }
    let request = SearchListingSourcesRequest {
        search: ListingSourceSearch {
            name_query: Some(text("anchor")),
            ..ListingSourceSearch::default()
        },
        sort: Some(Sort {
            sort: SortListingSourceField::Name,
            order: SortOrder::Asc,
        }),
        cursor: Some(Cursor {
            size: 1,
            search_after: None,
        }),
    };

    let first_page = search(&pool, &request).await;
    assert_eq!(vec![ids[0]], source_ids(&first_page));
    delete_source(&pool, ids[0]).await;

    let second_page = search(&pool, &next(&request, first_page)).await;
    assert_eq!(vec![ids[1]], source_ids(&second_page));
    rename_source(&pool, ids[1], "Renamed B").await;

    let third_page = search(&pool, &next(&request, second_page)).await;
    assert_eq!(vec![ids[2]], source_ids(&third_page));
    delete_source(&pool, ids[3]).await;

    let last_page = search(&pool, &next(&request, third_page)).await;
    assert_eq!(vec![ids[4]], source_ids(&last_page));
    assert!(last_page.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_traverse_duplicate_sort_values_deterministically_in_both_directions() {
    let pool = get_postgres_client().await;
    let operator = insert_operator(&pool).await;
    for (sequence, name, created) in [
        (4, "Order Twin", datetime!(2026-01-02 00:00 UTC)),
        (2, "Order Twin", datetime!(2026-01-01 00:00 UTC)),
        (5, "Order Last", datetime!(2026-01-02 00:00 UTC)),
        (1, "Order First", datetime!(2026-01-01 00:00 UTC)),
        (3, "Order Twin", datetime!(2026-01-02 00:00 UTC)),
    ] {
        insert_source(&pool, ordered_source_id(sequence), name, operator, created).await;
    }

    for sort in [
        SortListingSourceField::Name,
        SortListingSourceField::Slug,
        SortListingSourceField::Created,
    ] {
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let request = SearchListingSourcesRequest {
                search: ListingSourceSearch {
                    operator_party_id: Some(operator),
                    ..ListingSourceSearch::default()
                },
                sort: Some(Sort { sort, order }),
                cursor: Some(Cursor {
                    size: 100,
                    search_after: None,
                }),
            };
            let whole = source_ids(&search(&pool, &request).await);
            let mut paged = Vec::new();
            let mut page = search(
                &pool,
                &SearchListingSourcesRequest {
                    cursor: Some(Cursor {
                        size: 1,
                        search_after: None,
                    }),
                    ..request.clone()
                },
            )
            .await;
            loop {
                paged.extend(source_ids(&page));
                if page.cursor.search_after.is_none() {
                    break;
                }
                page = search(&pool, &next(&request, page)).await;
            }

            assert_eq!(5, whole.len(), "{sort:?} {order:?}");
            assert_eq!(whole, paged, "{sort:?} {order:?}");
        }
    }
    let created_desc = SearchListingSourcesRequest {
        search: ListingSourceSearch {
            operator_party_id: Some(operator),
            ..ListingSourceSearch::default()
        },
        sort: Some(Sort {
            sort: SortListingSourceField::Created,
            order: SortOrder::Desc,
        }),
        cursor: None,
    };
    assert_eq!(
        [3, 4, 5, 1, 2].map(ordered_source_id).to_vec(),
        source_ids(&search(&pool, &created_desc).await)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_filter_by_the_country_where_the_source_is_located() {
    let pool = get_postgres_client().await;
    let operator = insert_operator(&pool).await;
    let ids: [ListingSourceId; 11] = std::array::from_fn(|index| ordered_source_id(index as u128));
    for id in ids {
        insert_source(
            &pool,
            id,
            "Located",
            operator,
            datetime!(2026-01-01 00:00 UTC),
        )
        .await;
    }
    let business = Evidence::Business;
    let inventory = Evidence::Inventory;
    let site = |country: Option<&str>| Some(description("Supplied", country));

    let business_de = insert_location(&pool, operator, site(Some("DE")), "ACTIVE", 1).await;
    select_location(&pool, ids[0], operator, business_de, &[business]).await;
    let inventory_de = insert_location(&pool, operator, site(Some("DE")), "ACTIVE", 1).await;
    select_location(&pool, ids[1], operator, inventory_de, &[inventory]).await;
    let geocoded_de = insert_location(&pool, operator, site(None), "ACTIVE", 1).await;
    bind_resolution(&pool, geocoded_de, 1, Resolution::administrative("DE")).await;
    select_location(&pool, ids[2], operator, geocoded_de, &[business]).await;
    let stale_geocoded_de = insert_location(&pool, operator, site(None), "ACTIVE", 2).await;
    bind_resolution(
        &pool,
        stale_geocoded_de,
        1,
        Resolution::administrative("DE"),
    )
    .await;
    select_location(&pool, ids[3], operator, stale_geocoded_de, &[business]).await;
    let without_evidence = insert_location(&pool, operator, site(Some("DE")), "ACTIVE", 1).await;
    select_location(&pool, ids[4], operator, without_evidence, &[]).await;
    let retired = insert_location(&pool, operator, site(Some("DE")), "RETIRED", 1).await;
    select_location(&pool, ids[5], operator, retired, &[business, inventory]).await;
    let france = insert_location(&pool, operator, site(Some("FR")), "ACTIVE", 1).await;
    select_location(&pool, ids[6], operator, france, &[business]).await;
    // ids[7] selects no location default.
    let submitted_over_geocoded =
        insert_location(&pool, operator, site(Some("FR")), "ACTIVE", 1).await;
    bind_resolution(
        &pool,
        submitted_over_geocoded,
        1,
        Resolution::administrative("DE"),
    )
    .await;
    select_location(
        &pool,
        ids[8],
        operator,
        submitted_over_geocoded,
        &[inventory],
    )
    .await;
    let reviewed = insert_location(&pool, operator, site(None), "ACTIVE", 1).await;
    bind_resolution(&pool, reviewed, 1, Resolution::reviewed(["IT", "AT"], 1, 2)).await;
    select_location(&pool, ids[9], operator, reviewed, &[business]).await;
    let stale_review = insert_location(&pool, operator, site(None), "ACTIVE", 1).await;
    bind_resolution(
        &pool,
        stale_review,
        1,
        Resolution::reviewed(["IT", "AT"], 1, 1),
    )
    .await;
    select_location(&pool, ids[10], operator, stale_review, &[business]).await;

    let in_country = |country| SearchListingSourcesRequest {
        search: ListingSourceSearch {
            operator_party_id: Some(operator),
            country: Some(country),
            ..ListingSourceSearch::default()
        },
        sort: None,
        cursor: None,
    };
    for (country, expected) in [
        (CountryCode::DEU, vec![ids[0], ids[1], ids[2]]),
        (CountryCode::FRA, vec![ids[6], ids[8]]),
        (CountryCode::AUT, vec![ids[9]]),
        (CountryCode::ITA, vec![]),
    ] {
        assert_eq!(
            expected,
            source_ids(&search(&pool, &in_country(country)).await),
            "{country:?}"
        );
    }
}

async fn search(
    pool: &sqlx::PgPool,
    request: &SearchListingSourcesRequest,
) -> SearchListingSourcesResult {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin listing source search transaction: {error}"));
    let result = SqlxListingSourceSearchReaderFactory::new()
        .in_transaction(&mut tx)
        .search(request)
        .await
        .unwrap_or_else(|error| panic!("search listing sources: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit listing source search transaction: {error}"));
    result
}

fn next(
    request: &SearchListingSourcesRequest,
    page: SearchListingSourcesResult,
) -> SearchListingSourcesRequest {
    SearchListingSourcesRequest {
        cursor: Some(page.cursor),
        ..request.clone()
    }
}

fn source_ids(page: &SearchListingSourcesResult) -> Vec<ListingSourceId> {
    page.items
        .iter()
        .map(|item| item.listing_source_id)
        .collect()
}

async fn insert_operator(pool: &sqlx::PgPool) -> party_core::party_id::PartyId {
    let party_id = party_core::party_id::PartyId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id.into_uuid())
        .bind(format!("search-operator-{}", party_id.as_uuid().simple()))
        .bind("Search Operator")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test party: {error}"));
    party_id
}

async fn insert_source(
    pool: &sqlx::PgPool,
    id: ListingSourceId,
    name: &str,
    operator: party_core::party_id::PartyId,
    created: OffsetDateTime,
) {
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, created, updated, listing_source_type) VALUES ($1, $2, $3, $4, $5, $5, 'DEALER')")
        .bind(id.into_uuid())
        .bind(format!("search-source-{}", id.as_uuid().simple()))
        .bind(name)
        .bind(operator.into_uuid())
        .bind(created)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test source: {error}"));
}

#[derive(Clone, Copy)]
enum Evidence {
    Business,
    Inventory,
}

async fn insert_location(
    pool: &sqlx::PgPool,
    operator: party_core::party_id::PartyId,
    geography: Option<serde_json::Value>,
    lifecycle: &str,
    revision: i64,
) -> uuid::Uuid {
    let location_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO party_locations (party_location_id, party_id, label, geography, lifecycle, revision, input_revision, created_actor, updated_actor) VALUES ($1, $2, 'Site', $3, $4, $5, 1, 'SERVICE:test', 'SERVICE:test')")
        .bind(location_id)
        .bind(operator.into_uuid())
        .bind(geography)
        .bind(lifecycle)
        .bind(revision)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test party location: {error}"));
    location_id
}

async fn select_location(
    pool: &sqlx::PgPool,
    source: ListingSourceId,
    operator: party_core::party_id::PartyId,
    location_id: uuid::Uuid,
    evidence: &[Evidence],
) {
    let scoped = |kind: Evidence, scope: &str| {
        evidence
            .iter()
            .any(|present| std::mem::discriminant(present) == std::mem::discriminant(&kind))
            .then(|| json!({"scope": scope, "reference": "source catalogue"}))
    };
    sqlx::query("INSERT INTO listing_source_location_defaults (listing_source_id, party_id, party_location_id, business_evidence, inventory_evidence, revision, updated_actor) VALUES ($1, $2, $3, $4, $5, 2, 'SERVICE:test')")
        .bind(source.into_uuid())
        .bind(operator.into_uuid())
        .bind(location_id)
        .bind(scoped(Evidence::Business, "ASSOCIATED_BUSINESS"))
        .bind(scoped(Evidence::Inventory, "ADVERTISED_INVENTORY"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test location default: {error}"));
}

fn description(address_text: &str, country: Option<&str>) -> serde_json::Value {
    json!({"reference_release": "iso-codes-4.20.1", "address_text": address_text, "country": country, "subdivision": null, "postal_code": null})
}

const GEOCODING_PROFILE: &str = "LISTING_SOURCE_SEARCH_TEST";

/// Stored geocoding outcome in the shapes `geo-postgres` persists. The filter reads only the
/// evidence and binding, so the fixture omits the replayable resolution input.
struct Resolution {
    status: &'static str,
    generation: i64,
    evidence: serde_json::Value,
    selection: Option<(i32, i64)>,
}

impl Resolution {
    /// Administrative acceptance retains the source candidate and accepts its projection.
    fn administrative(country: &str) -> Self {
        Self {
            status: "ACCEPTED",
            generation: 1,
            evidence: json!({
                "status": "ACCEPTED",
                "accepted_index": 1,
                "candidates": [
                    candidate(description("Provider address", Some(country))),
                    candidate(json!({"reference_release": "iso-codes-4.20.1", "address_text": null, "country": country, "subdivision": null, "postal_code": null})),
                ],
            }),
            selection: None,
        }
    }

    /// Ambiguous evidence with an administrator's selection made at `selected_generation`.
    fn reviewed(countries: [&str; 2], selected: i32, selected_generation: i64) -> Self {
        Self {
            status: "AMBIGUOUS",
            generation: 2,
            evidence: json!({
                "status": "AMBIGUOUS",
                "accepted_index": null,
                "candidates": countries.map(|country| candidate(description("Provider address", Some(country)))),
            }),
            selection: Some((selected, selected_generation)),
        }
    }
}

fn candidate(description: serde_json::Value) -> serde_json::Value {
    json!({
        "description": description,
        "position": null,
        "acceptable": true,
        "provider": "TEST",
        "product": "TEST",
        "data_source": "TEST",
        "region": "TEST",
        "profile": GEOCODING_PROFILE,
        "external_reference": "synthetic-site",
        "attribution_url": "https://example.test/attribution"
    })
}

/// Binds `resolution` to the location's state at `guard`.
async fn bind_resolution(
    pool: &sqlx::PgPool,
    location_id: uuid::Uuid,
    guard: i64,
    resolution: Resolution,
) {
    sqlx::query(
        "INSERT INTO geo_resolution_provider_controls (profile) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(GEOCODING_PROFILE)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert test geocoding profile: {error}"));
    let resolution_id = uuid::Uuid::now_v7();
    let fingerprint = resolution_id.simple().to_string().repeat(2);
    sqlx::query("INSERT INTO geo_resolutions (resolution_id, fingerprint, profile, policy_version, scope_key, input, generation, status, evidence) VALUES ($1, $2, $3, 'PRECISION_FIRST_V1', $2, '{}', $4, $5, $6)")
        .bind(resolution_id)
        .bind(fingerprint)
        .bind(GEOCODING_PROFILE)
        .bind(resolution.generation)
        .bind(resolution.status)
        .bind(resolution.evidence)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test geocoding resolution: {error}"));
    let (selected_candidate, selected_generation) = resolution.selection.unzip();
    sqlx::query("INSERT INTO geo_resolution_bindings (subject_kind, subject_id, resolution_id, input_revision, guard, selected_candidate, selected_generation, reviewed_actor) VALUES ('PARTY_LOCATION', $1, $2, 1, $3, $4, $5, CASE WHEN $4::integer IS NULL THEN NULL ELSE 'USER:test' END)")
        .bind(location_id)
        .bind(resolution_id)
        .bind(guard)
        .bind(selected_candidate)
        .bind(selected_generation)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("bind test geocoding resolution: {error}"));
}

async fn delete_source(pool: &sqlx::PgPool, id: ListingSourceId) {
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
        .bind(id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("delete test source: {error}"));
}

async fn rename_source(pool: &sqlx::PgPool, id: ListingSourceId, name: &str) {
    sqlx::query("UPDATE listing_sources SET name = $2 WHERE listing_source_id = $1")
        .bind(id.into_uuid())
        .bind(name)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("rename test source: {error}"));
}

fn ordered_source_id(sequence: u128) -> ListingSourceId {
    let uuid = uuid::Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0000 | sequence);
    ListingSourceId::try_from(uuid)
        .unwrap_or_else(|error| panic!("invalid ordered ListingSource ID fixture: {error}"))
}

fn text(value: &str) -> TextQuery<0> {
    TextQuery::try_from(value).unwrap_or_else(|error| panic!("invalid search query: {error}"))
}
