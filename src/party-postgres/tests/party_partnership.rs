//! Administrator Party reads expose only the Party's active Partnership.

use application::transaction::{Transaction, UnitOfWork};
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use party_core::party_search::PartySearch;
use party_postgres::{SqlxPartyPartnershipReaderFactory, SqlxPartySearchReaderFactory};
use party_service::ports::{
    PartyPartnershipReader, PartyPartnershipReaderFactory, PartySearchReader,
    PartySearchReaderFactory,
};
use party_service::use_cases::queries::search_parties::SearchPartiesRequest;
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_expose_only_the_active_partnership_of_a_party() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let active_partnership = PartnershipId::new();
    let active = insert_party(&pool, "active", Some((active_partnership, "ACTIVE"))).await;
    let dissolved = insert_party(
        &pool,
        "dissolved",
        Some((PartnershipId::new(), "DISSOLVED")),
    )
    .await;
    let unrelated = insert_party(&pool, "none", None).await;

    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin party read transaction: {error}"));
    let searched = SqlxPartySearchReaderFactory::new()
        .in_transaction(&mut tx)
        .search(&SearchPartiesRequest {
            search: PartySearch::default(),
            sort: None,
            cursor: None,
        })
        .await
        .unwrap_or_else(|error| panic!("search parties: {error:?}"))
        .items;
    let partnerships = SqlxPartyPartnershipReaderFactory::new();
    for (party_id, expected) in [
        (active, Some(active_partnership)),
        (dissolved, None),
        (unrelated, None),
    ] {
        let read = partnerships
            .in_transaction(&mut tx)
            .find_active_partnership_id(party_id)
            .await
            .unwrap_or_else(|error| panic!("read party partnership: {error:?}"));
        let in_search = searched
            .iter()
            .find(|summary| summary.party_id == party_id)
            .map(|summary| summary.partnership_id)
            .unwrap_or_else(|| panic!("party should be searchable"));

        assert_eq!(expected, read);
        assert_eq!(expected, in_search);
    }
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit party read transaction: {error}"));
}

async fn insert_party(
    pool: &sqlx::PgPool,
    slug: &str,
    partnership: Option<(PartnershipId, &str)>,
) -> PartyId {
    let party_id = PartyId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id.into_uuid())
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert party fixture: {error}"));
    if let Some((partnership_id, business_state)) = partnership {
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
        )
        .bind(partnership_id.into_uuid())
        .bind(party_id.into_uuid())
        .bind(business_state)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert partnership fixture: {error}"));
    }
    party_id
}
