use application::{
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_postgres::SqlxPartnershipSearchReaderFactory;
use partnership_service::{
    ports::{PartnershipSearchReader, PartnershipSearchReaderFactory},
    use_cases::queries::list_admin_partnerships::{
        ListAdminPartnershipsRequest, ListAdminPartnershipsResult,
    },
};
use party_core::party_id::PartyId;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::macros::datetime;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn seed_user(pool: &PgPool) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@partnership-reader.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader user: {error}"));
    user_id
}

async fn seed_party(pool: &PgPool, name: &str) -> PartyId {
    let party_id = PartyId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id.into_uuid())
        .bind(format!("party-{}", party_id.as_uuid().simple()))
        .bind(name)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader party: {error}"));
    party_id
}

async fn seed_partnership(
    pool: &PgPool,
    party_id: PartyId,
    business_state: &str,
    created: time::OffsetDateTime,
) -> PartnershipId {
    let partnership_id = PartnershipId::new();
    sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state, created, updated) VALUES ($1, $2, $3, $4, $4)",
        )
        .bind(partnership_id.into_uuid())
        .bind(party_id.into_uuid())
        .bind(business_state)
        .bind(created)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader partnership: {error}"));
    partnership_id
}

async fn seed_listing_source(pool: &PgPool, operator_party_id: PartyId) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(listing_source_id.into_uuid())
        .bind(format!("source-{}", listing_source_id.as_uuid().simple()))
        .bind("Reader source")
        .bind(operator_party_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader listing source: {error}"));
    listing_source_id
}

async fn add_member(pool: &PgPool, user_id: UserId, partnership_id: PartnershipId) {
    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(user_id.into_uuid())
        .bind(partnership_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader membership: {error}"));
}

async fn add_grant(
    pool: &PgPool,
    partnership_id: PartnershipId,
    listing_source_id: ListingSourceId,
) {
    sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(partnership_id.into_uuid())
        .bind(listing_source_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader source grant: {error}"));
}

async fn read_page(request: &ListAdminPartnershipsRequest) -> ListAdminPartnershipsResult {
    let pool = get_postgres_client().await;
    let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin reader transaction: {error}"));
    let result = SqlxPartnershipSearchReaderFactory::new()
        .in_transaction(&mut tx)
        .search(request)
        .await;
    match result {
        Ok(result) => {
            tx.commit()
                .await
                .unwrap_or_else(|error| panic!("failed to commit reader transaction: {error}"));
            result
        }
        Err(error) => panic!("reader failed: {error}"),
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_filter_by_party_member_and_listing_source_without_changing_counts() {
    let pool = get_postgres_client().await;
    let matching_party = seed_party(&pool, "Matching party").await;
    let other_party = seed_party(&pool, "Other party").await;
    let matching_user = seed_user(&pool).await;
    let second_matching_user = seed_user(&pool).await;
    let other_user = seed_user(&pool).await;
    let matching_partnership = seed_partnership(
        &pool,
        matching_party,
        "ACTIVE",
        datetime!(2026-01-02 00:00 UTC),
    )
    .await;
    let other_partnership = seed_partnership(
        &pool,
        other_party,
        "ACTIVE",
        datetime!(2026-01-01 00:00 UTC),
    )
    .await;
    let matching_source = seed_listing_source(&pool, matching_party).await;
    let second_matching_source = seed_listing_source(&pool, matching_party).await;
    let other_source = seed_listing_source(&pool, other_party).await;
    add_member(&pool, matching_user, matching_partnership).await;
    add_member(&pool, second_matching_user, matching_partnership).await;
    add_member(&pool, other_user, other_partnership).await;
    add_grant(&pool, matching_partnership, matching_source).await;
    add_grant(&pool, matching_partnership, second_matching_source).await;
    add_grant(&pool, other_partnership, other_source).await;

    let mut request = ListAdminPartnershipsRequest {
        party_id: Some(matching_party),
        member_user_id: None,
        listing_source_id: None,
        cursor: Some(Cursor {
            size: 10,
            search_after: None,
        }),
    };
    let by_party = read_page(&request).await;
    assert_eq!(
        vec![matching_partnership],
        by_party
            .items
            .iter()
            .map(|item| item.partnership_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(2, by_party.items[0].member_count);
    assert_eq!(2, by_party.items[0].listing_source_grant_count);

    request.party_id = None;
    request.member_user_id = Some(matching_user);
    let by_member = read_page(&request).await;
    assert_eq!(
        vec![matching_partnership],
        by_member
            .items
            .iter()
            .map(|item| item.partnership_id)
            .collect::<Vec<_>>()
    );

    request.member_user_id = None;
    request.listing_source_id = Some(matching_source);
    let by_source = read_page(&request).await;
    assert_eq!(
        vec![matching_partnership],
        by_source
            .items
            .iter()
            .map(|item| item.partnership_id)
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_partnerships_in_created_descending_order_with_tied_cursor() {
    let pool = get_postgres_client().await;
    let created = datetime!(2026-05-05 12:00 UTC);
    let mut ids = Vec::new();
    for index in 0..3 {
        let party = seed_party(&pool, &format!("Tied party {index}")).await;
        ids.push(seed_partnership(&pool, party, "ACTIVE", created).await);
    }
    let mut expected_ids = ids;
    expected_ids.sort_by(|left, right| right.cmp(left));

    let request = ListAdminPartnershipsRequest {
        cursor: Some(Cursor {
            size: 2,
            search_after: None,
        }),
        ..Default::default()
    };
    let first = read_page(&request).await;
    assert_eq!(
        expected_ids[..2].to_vec(),
        first
            .items
            .iter()
            .map(|item| item.partnership_id)
            .collect::<Vec<_>>()
    );
    let first_cursor = first
        .cursor
        .search_after
        .unwrap_or_else(|| panic!("first page should have a continuation cursor"));

    let second = read_page(&ListAdminPartnershipsRequest {
        cursor: Some(Cursor {
            size: 2,
            search_after: Some(first_cursor),
        }),
        ..request
    })
    .await;

    assert_eq!(1, second.items.len());
    assert_eq!(expected_ids[2], second.items[0].partnership_id);
    assert!(second.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_exclude_dissolved_partnerships_from_admin_search() {
    let pool = get_postgres_client().await;
    let active_party = seed_party(&pool, "Active partnership party").await;
    let dissolved_party = seed_party(&pool, "Dissolved partnership party").await;
    let active = seed_partnership(
        &pool,
        active_party,
        "ACTIVE",
        datetime!(2026-06-02 00:00 UTC),
    )
    .await;
    seed_partnership(
        &pool,
        dissolved_party,
        "DISSOLVED",
        datetime!(2026-06-03 00:00 UTC),
    )
    .await;

    let result = read_page(&ListAdminPartnershipsRequest {
        cursor: Some(Cursor {
            size: 10,
            search_after: None,
        }),
        ..Default::default()
    })
    .await;

    assert_eq!(
        vec![active],
        result
            .items
            .iter()
            .map(|item| item.partnership_id)
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_an_empty_partnership_page() {
    let result = read_page(&ListAdminPartnershipsRequest {
        cursor: Some(Cursor {
            size: 2,
            search_after: None,
        }),
        ..Default::default()
    })
    .await;

    assert!(result.items.is_empty());
    assert!(result.cursor.search_after.is_none());
}
