use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use listing_source_core::{ListingSourceId, ListingSourceSearch, SortListingSourceField};
use listing_source_postgres::SqlxListingSourceSearchReaderFactory;
use listing_source_service::ports::{ListingSourceSearchReader, ListingSourceSearchReaderFactory};
use listing_source_service::use_cases::queries::search_listing_sources::{
    SearchListingSourcesRequest, SearchListingSourcesResult,
};
use platform_postgres::SqlxUnitOfWork;
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
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, created, updated) VALUES ($1, $2, $3, $4, $5, $5)")
        .bind(id.into_uuid())
        .bind(format!("search-source-{}", id.as_uuid().simple()))
        .bind(name)
        .bind(operator.into_uuid())
        .bind(created)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test source: {error}"));
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
