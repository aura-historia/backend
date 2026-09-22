use application::{
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::{
    query::range_query::RangeQuery,
    sort::{Sort, SortOrder},
};
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_application_id::PartnershipApplicationId,
    partnership_application_search::PartnershipApplicationSearch,
    partnership_application_state::PartnershipApplicationState,
    partnership_proposal_type::PartnershipProposalType,
    sort_partnership_application_field::SortPartnershipApplicationField,
};
use partnership_postgres::SqlxPartnershipApplicationReaderFactory;
use partnership_service::{
    ports::{PartnershipApplicationReader, PartnershipApplicationReaderFactory},
    use_cases::queries::list_admin_partnership_applications::{
        ListAdminPartnershipApplicationsRequest, ListAdminPartnershipApplicationsResult,
    },
};
use serde_json::json;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn seed_user(pool: &PgPool) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@reader.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader user: {error}"));
    user_id
}

async fn seed_application(
    pool: &PgPool,
    applicant_user_id: UserId,
    state: &str,
    proposal: serde_json::Value,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> PartnershipApplicationId {
    ensure_existing_listing_source_proposal(pool, &proposal).await;
    let application_id = PartnershipApplicationId::new();
    sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, created, updated) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(application_id.into_uuid())
        .bind(applicant_user_id.into_uuid())
        .bind(state)
        .bind(proposal)
        .bind(created)
        .bind(updated)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader application: {error}"));
    application_id
}

async fn read_page(
    request: &ListAdminPartnershipApplicationsRequest,
) -> ListAdminPartnershipApplicationsResult {
    let pool = get_postgres_client().await;
    let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin reader transaction: {error}"));
    let result = SqlxPartnershipApplicationReaderFactory::new()
        .in_transaction(&mut tx)
        .search_admin(request)
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

async fn ensure_existing_listing_source_proposal(pool: &PgPool, proposal: &serde_json::Value) {
    if proposal.get("type").and_then(serde_json::Value::as_str) != Some("EXISTING_LISTING_SOURCE") {
        return;
    }
    let listing_source_uuid = proposal
        .get("listing_source_id")
        .cloned()
        .map(serde_json::from_value::<uuid::Uuid>)
        .transpose()
        .unwrap_or_else(|error| panic!("invalid existing proposal ListingSource UUID: {error}"))
        .unwrap_or_else(|| panic!("existing proposal must contain a ListingSource UUID"));
    let listing_source_id = ListingSourceId::try_from(listing_source_uuid)
        .unwrap_or_else(|error| panic!("invalid existing proposal ListingSource ID: {error}"));
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to check reader ListingSource: {error}"));
    if exists {
        return;
    }

    let party_id = party_core::party_id::PartyId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id.into_uuid())
        .bind(format!("reader-party-{}", party_id.as_uuid().simple()))
        .bind(format!("Reader Party {party_id}"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader Party: {error}"));
    sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(listing_source_id.into_uuid())
        .bind(format!(
                    "reader-source-{}",
                    listing_source_id.as_uuid().simple()
                ))
        .bind(format!("Reader ListingSource {listing_source_id}"))
        .bind(party_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader ListingSource: {error}"));
}

fn existing_proposal(listing_source_id: ListingSourceId) -> serde_json::Value {
    json!({
        "type": "EXISTING_LISTING_SOURCE",
        "listing_source_id": listing_source_id.into_uuid(),
    })
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_admin_application_filters_and_inclusive_ranges() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let listing_source_id = ListingSourceId::new();
    let matching_id = seed_application(
        &pool,
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(listing_source_id),
        datetime!(2026-01-01 00:00 UTC),
        datetime!(2026-02-01 00:00 UTC),
    )
    .await;
    let _other_id = seed_application(
        &pool,
        applicant_user_id,
        "IN_REVIEW",
        existing_proposal(ListingSourceId::new()),
        datetime!(2026-01-02 00:00 UTC),
        datetime!(2026-02-02 00:00 UTC),
    )
    .await;
    let mut search = PartnershipApplicationSearch::default();
    search
        .state_query
        .extend([PartnershipApplicationState::Submitted]);
    search
        .proposal_type_query
        .extend([PartnershipProposalType::ExistingListingSource]);
    search.applicant_user_id = Some(applicant_user_id);
    search.listing_source_id = Some(listing_source_id);
    search.created = Some(RangeQuery {
        min: Some(datetime!(2026-01-01 00:00 UTC)),
        max: Some(datetime!(2026-01-01 00:00 UTC)),
    });
    search.updated = Some(RangeQuery {
        min: Some(datetime!(2026-02-01 00:00 UTC)),
        max: Some(datetime!(2026-02-01 00:00 UTC)),
    });
    let request = ListAdminPartnershipApplicationsRequest {
        search,
        sort: Some(Sort {
            sort: SortPartnershipApplicationField::Updated,
            order: SortOrder::Desc,
        }),
        cursor: Some(Cursor {
            size: 100,
            search_after: None,
        }),
    };

    let result = read_page(&request).await;

    assert_eq!(1, result.items.len());
    assert_eq!(matching_id, result.items[0].id);
    assert_eq!(datetime!(2026-01-01 00:00 UTC), result.items[0].created);
    assert_eq!(datetime!(2026-02-01 00:00 UTC), result.items[0].updated);
    assert!(result.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_follow_descending_cursor_through_tied_created_timestamps() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let created = datetime!(2026-05-05 12:00 UTC);
    let ids = vec![
        seed_application(
            &pool,
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(ListingSourceId::new()),
            created,
            created,
        )
        .await,
        seed_application(
            &pool,
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(ListingSourceId::new()),
            created,
            created,
        )
        .await,
        seed_application(
            &pool,
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(ListingSourceId::new()),
            created,
            created,
        )
        .await,
    ];
    let mut expected_ids = ids.clone();
    expected_ids.sort_by(|left, right| right.cmp(left));
    let request = ListAdminPartnershipApplicationsRequest {
        search: PartnershipApplicationSearch::default(),
        sort: None,
        cursor: Some(Cursor {
            size: 2,
            search_after: None,
        }),
    };

    let first = read_page(&request).await;
    assert_eq!(
        expected_ids[..2].to_vec(),
        first.items.iter().map(|item| item.id).collect::<Vec<_>>()
    );
    let first_cursor = first
        .cursor
        .search_after
        .unwrap_or_else(|| panic!("first page should have a continuation cursor"));
    let second_request = ListAdminPartnershipApplicationsRequest {
        cursor: Some(Cursor {
            size: 2,
            search_after: Some(first_cursor),
        }),
        ..request
    };

    let second = read_page(&second_request).await;

    assert_eq!(1, second.items.len());
    assert_eq!(expected_ids[2], second.items[0].id);
    assert!(second.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_an_empty_admin_application_page() {
    let request = ListAdminPartnershipApplicationsRequest {
        search: PartnershipApplicationSearch::default(),
        sort: None,
        cursor: Some(Cursor {
            size: 2,
            search_after: None,
        }),
    };

    let result = read_page(&request).await;

    assert!(result.items.is_empty());
    assert!(result.cursor.search_after.is_none());
}
