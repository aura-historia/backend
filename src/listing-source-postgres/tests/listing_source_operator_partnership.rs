//! Administrator ListingSource reads expose the operator Party's active Partnership.

use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::{ListingSourceId, ListingSourceSearch, ListingSourceSlugId};
use listing_source_postgres::{SqlxListingSourceReaders, SqlxListingSourceSearchReaderFactory};
use listing_source_service::ports::{
    ListingSourceDetailsReader, ListingSourceSearchReader, ListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesRequest;
use partnership_core::partnership_id::PartnershipId;
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_expose_only_the_operators_active_partnership() {
    let pool = get_postgres_client().await;
    let active_partnership = PartnershipId::new();
    let active = insert_source(
        &pool,
        "operator-active",
        Some((active_partnership, "ACTIVE")),
    )
    .await;
    let dissolved = insert_source(
        &pool,
        "operator-dissolved",
        Some((PartnershipId::new(), "DISSOLVED")),
    )
    .await;
    let unrelated = insert_source(&pool, "operator-none", None).await;
    let readers = SqlxListingSourceReaders::new(pool.clone());
    let searched = search_all(&pool).await;

    for (id, slug, expected) in [
        (active, "operator-active", Some(active_partnership)),
        (dissolved, "operator-dissolved", None),
        (unrelated, "operator-none", None),
    ] {
        let by_id = readers
            .find_details_by_id(id)
            .await
            .unwrap_or_else(|error| panic!("read {slug} by ID: {error}"))
            .unwrap_or_else(|| panic!("{slug} should exist"));
        let by_slug = readers
            .find_details_by_slug(
                &ListingSourceSlugId::raw(slug).unwrap_or_else(|error| panic!("{error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("read {slug} by slug: {error}"))
            .unwrap_or_else(|| panic!("{slug} should exist"));
        let in_search = searched
            .iter()
            .find(|(source_id, _)| *source_id == id)
            .map(|(_, partnership_id)| *partnership_id)
            .unwrap_or_else(|| panic!("{slug} should be searchable"));

        assert_eq!(expected, by_id.operator_partnership_id, "{slug} by ID");
        assert_eq!(expected, by_slug.operator_partnership_id, "{slug} by slug");
        assert_eq!(expected, in_search, "{slug} in search");
    }
}

async fn search_all(pool: &sqlx::PgPool) -> Vec<(ListingSourceId, Option<PartnershipId>)> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin listing source search transaction: {error}"));
    let result = SqlxListingSourceSearchReaderFactory::new()
        .in_transaction(&mut tx)
        .search(&SearchListingSourcesRequest {
            search: ListingSourceSearch::default(),
            sort: None,
            cursor: Some(Cursor {
                size: 100,
                search_after: None,
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("search listing sources: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit listing source search transaction: {error}"));
    result
        .items
        .into_iter()
        .map(|item| (item.listing_source_id, item.operator.partnership_id))
        .collect()
}

async fn insert_source(
    pool: &sqlx::PgPool,
    slug: &str,
    partnership: Option<(PartnershipId, &str)>,
) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test party: {error}"));
    if let Some((partnership_id, business_state)) = partnership {
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
        )
        .bind(partnership_id.into_uuid())
        .bind(party_id)
        .bind(business_state)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test partnership: {error}"));
    }
    let id = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')")
        .bind(id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test source: {error}"));
    id
}
