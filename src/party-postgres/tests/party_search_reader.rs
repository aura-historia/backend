use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use party_core::party_id::PartyId;
use party_core::party_search::PartySearch;
use party_core::sort_party_field::SortPartyField;
use party_postgres::SqlxPartySearchReaderFactory;
use party_service::ports::{PartySearchReader, PartySearchReaderFactory};
use party_service::use_cases::queries::search_parties::SearchPartiesRequest;
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::macros::datetime;
use uuid::Uuid;

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_search_parties_with_filters_bounded_pages_and_stable_cursor() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let reader_factory = SqlxPartySearchReaderFactory::new();
    let first_id = ordered_party_id(1);
    let second_id = ordered_party_id(2);
    let third_id = ordered_party_id(3);

    insert_party(
        &pool,
        first_id,
        "Same Party",
        Some("+49 30 100001"),
        Some("first@example.test"),
        datetime!(2026-01-01 00:00 UTC),
        datetime!(2026-01-02 00:00 UTC),
    )
    .await;
    insert_party(
        &pool,
        second_id,
        "Same Party",
        Some("+49 30 100002"),
        Some("second@example.test"),
        datetime!(2026-02-01 00:00 UTC),
        datetime!(2026-02-02 00:00 UTC),
    )
    .await;
    insert_party(
        &pool,
        third_id,
        "Same Party",
        Some("+49 30 100003"),
        Some("third@example.test"),
        datetime!(2026-03-01 00:00 UTC),
        datetime!(2026-03-02 00:00 UTC),
    )
    .await;

    let request = SearchPartiesRequest {
        search: PartySearch {
            name_query: Some(text("same")),
            ..Default::default()
        },
        sort: Some(Sort {
            sort: SortPartyField::Name,
            order: SortOrder::Asc,
        }),
        cursor: Some(Cursor {
            size: 2,
            search_after: None,
        }),
    };
    let first_page = search(&unit_of_work, &reader_factory, &request).await;

    assert_eq!(2, first_page.items.len());
    assert_eq!(first_id, first_page.items[0].party_id);
    assert_eq!(second_id, first_page.items[1].party_id);
    assert_eq!(
        Some(second_id),
        first_page
            .cursor
            .search_after
            .as_ref()
            .map(|cursor| cursor.id)
    );

    let next_request = SearchPartiesRequest {
        cursor: Some(first_page.cursor),
        ..request
    };
    let second_page = search(&unit_of_work, &reader_factory, &next_request).await;

    assert_eq!(1, second_page.items.len());
    assert_eq!(third_id, second_page.items[0].party_id);
    assert!(second_page.cursor.search_after.is_none());

    let contact_request = SearchPartiesRequest {
        search: PartySearch {
            phone_query: Some(text("100002")),
            email_query: Some(text("second@example")),
            created: Some(RangeQuery {
                min: Some(datetime!(2026-02-01 00:00 UTC)),
                max: Some(datetime!(2026-02-01 00:00 UTC)),
            }),
            ..Default::default()
        },
        sort: None,
        cursor: Some(Cursor {
            size: 1000,
            search_after: None,
        }),
    };
    let contact_page = search(&unit_of_work, &reader_factory, &contact_request).await;

    assert_eq!(100, contact_page.cursor.size);
    assert_eq!(1, contact_page.items.len());
    assert_eq!(second_id, contact_page.items[0].party_id);
    assert_eq!(
        Some("+49 30 100002"),
        contact_page.items[0].contact.phone.as_deref()
    );
    assert_eq!(
        Some("second@example.test".to_owned()),
        contact_page.items[0]
            .contact
            .email
            .as_ref()
            .map(ToString::to_string)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_empty_party_search_result() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let reader_factory = SqlxPartySearchReaderFactory::new();
    let request = SearchPartiesRequest {
        search: PartySearch {
            email_query: Some(text("missing@example.test")),
            updated: Some(RangeQuery {
                min: Some(datetime!(2030-01-01 00:00 UTC)),
                max: None,
            }),
            ..Default::default()
        },
        sort: None,
        cursor: None,
    };

    let result = search(&unit_of_work, &reader_factory, &request).await;

    assert!(result.items.is_empty());
    assert!(result.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_continue_after_the_anchor_is_deleted_or_leaves_the_filter() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let reader_factory = SqlxPartySearchReaderFactory::new();
    let ids = [1, 2, 3, 4, 5].map(ordered_party_id);
    for (party_id, name) in ids
        .iter()
        .zip(["Anchor A", "Anchor B", "Anchor C", "Anchor D", "Anchor E"])
    {
        insert_named_party(&pool, *party_id, name, None).await;
    }
    let first_request = SearchPartiesRequest {
        search: PartySearch {
            name_query: Some(text("anchor")),
            ..Default::default()
        },
        sort: Some(Sort {
            sort: SortPartyField::Name,
            order: SortOrder::Asc,
        }),
        cursor: Some(Cursor {
            size: 1,
            search_after: None,
        }),
    };

    let first_page = search(&unit_of_work, &reader_factory, &first_request).await;
    assert_eq!(vec![ids[0]], party_ids(&first_page));
    delete_party(&pool, ids[0]).await;

    let second_page = search(
        &unit_of_work,
        &reader_factory,
        &next(&first_request, first_page),
    )
    .await;
    assert_eq!(vec![ids[1]], party_ids(&second_page));
    rename_party(&pool, ids[1], "Renamed B").await;

    let third_page = search(
        &unit_of_work,
        &reader_factory,
        &next(&first_request, second_page),
    )
    .await;
    assert_eq!(vec![ids[2]], party_ids(&third_page));
    delete_party(&pool, ids[3]).await;

    let last_page = search(
        &unit_of_work,
        &reader_factory,
        &next(&first_request, third_page),
    )
    .await;
    assert_eq!(vec![ids[4]], party_ids(&last_page));
    assert!(last_page.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_traverse_duplicate_and_missing_sort_values_deterministically_in_both_directions() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let reader_factory = SqlxPartySearchReaderFactory::new();
    for (sequence, name, email) in [
        (5, "Order Party", Some("same@example.test")),
        (2, "Order Party", None),
        (4, "Order Twin", Some("same@example.test")),
        (1, "Order Twin", Some("first@example.test")),
        (3, "Order Party", None),
        (6, "Order Last", Some("zulu@example.test")),
    ] {
        insert_named_party(&pool, ordered_party_id(sequence), name, email).await;
    }

    for sort in [
        SortPartyField::Name,
        SortPartyField::Email,
        SortPartyField::Phone,
    ] {
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let request = SearchPartiesRequest {
                search: PartySearch {
                    name_query: Some(text("order")),
                    ..Default::default()
                },
                sort: Some(Sort { sort, order }),
                cursor: Some(Cursor {
                    size: 100,
                    search_after: None,
                }),
            };
            let whole = party_ids(&search(&unit_of_work, &reader_factory, &request).await);
            let mut paged = Vec::new();
            let mut page = search(&unit_of_work, &reader_factory, &with_size(&request, 1)).await;
            loop {
                paged.extend(party_ids(&page));
                if page.cursor.search_after.is_none() {
                    break;
                }
                page = search(&unit_of_work, &reader_factory, &next(&request, page)).await;
            }

            assert_eq!(6, whole.len(), "{sort:?} {order:?}");
            assert_eq!(whole, paged, "{sort:?} {order:?}");
        }
    }
    let email_desc = SearchPartiesRequest {
        search: PartySearch {
            name_query: Some(text("order")),
            ..Default::default()
        },
        sort: Some(Sort {
            sort: SortPartyField::Email,
            order: SortOrder::Desc,
        }),
        cursor: None,
    };
    assert_eq!(
        [6, 4, 5, 1, 2, 3].map(ordered_party_id).to_vec(),
        party_ids(&search(&unit_of_work, &reader_factory, &email_desc).await)
    );
}

fn next(
    request: &SearchPartiesRequest,
    page: party_service::use_cases::queries::search_parties::SearchPartiesResult,
) -> SearchPartiesRequest {
    SearchPartiesRequest {
        cursor: Some(page.cursor),
        ..request.clone()
    }
}

fn with_size(request: &SearchPartiesRequest, size: u64) -> SearchPartiesRequest {
    SearchPartiesRequest {
        cursor: Some(Cursor {
            size,
            search_after: None,
        }),
        ..request.clone()
    }
}

fn party_ids(
    page: &party_service::use_cases::queries::search_parties::SearchPartiesResult,
) -> Vec<PartyId> {
    page.items.iter().map(|item| item.party_id).collect()
}

async fn search(
    unit_of_work: &SqlxUnitOfWork,
    reader_factory: &SqlxPartySearchReaderFactory,
    request: &SearchPartiesRequest,
) -> party_service::use_cases::queries::search_parties::SearchPartiesResult {
    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin party search transaction: {error}"),
    };
    let result = match reader_factory.in_transaction(&mut tx).search(request).await {
        Ok(result) => result,
        Err(error) => panic!("failed to search parties: {error:?}"),
    };
    if let Err(error) = tx.commit().await {
        panic!("failed to commit party search transaction: {error}");
    }
    result
}

async fn insert_party(
    pool: &sqlx::PgPool,
    party_id: PartyId,
    name: &str,
    phone: Option<&str>,
    email: Option<&str>,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
) {
    if let Err(error) = sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name, phone, email, created, updated) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(party_id.into_uuid())
    .bind(format!("same-party-{}", party_id.as_uuid().simple()))
    .bind(name)
    .bind(phone)
    .bind(email)
    .bind(created)
    .bind(updated)
    .execute(pool)
    .await
    {
        panic!("failed to insert party fixture: {error}");
    }
}

async fn insert_named_party(
    pool: &sqlx::PgPool,
    party_id: PartyId,
    name: &str,
    email: Option<&str>,
) {
    insert_party(
        pool,
        party_id,
        name,
        None,
        email,
        datetime!(2026-01-01 00:00 UTC),
        datetime!(2026-01-01 00:00 UTC),
    )
    .await;
}

async fn delete_party(pool: &sqlx::PgPool, party_id: PartyId) {
    if let Err(error) = sqlx::query("DELETE FROM parties WHERE party_id = $1")
        .bind(party_id.into_uuid())
        .execute(pool)
        .await
    {
        panic!("failed to delete party fixture: {error}");
    }
}

async fn rename_party(pool: &sqlx::PgPool, party_id: PartyId, name: &str) {
    if let Err(error) = sqlx::query("UPDATE parties SET name = $2 WHERE party_id = $1")
        .bind(party_id.into_uuid())
        .bind(name)
        .execute(pool)
        .await
    {
        panic!("failed to rename party fixture: {error}");
    }
}

fn ordered_party_id(sequence: u128) -> PartyId {
    let uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0000 | sequence);
    PartyId::try_from(uuid)
        .unwrap_or_else(|error| panic!("invalid ordered Party ID fixture: {error}"))
}

fn text(value: &str) -> TextQuery<0> {
    match TextQuery::try_from(value.to_owned()) {
        Ok(value) => value,
        Err(error) => panic!("invalid party search fixture query: {error}"),
    }
}
