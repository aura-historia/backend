use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::{ListingSourceId, ListingSourceSlugId};
use listing_source_postgres::SqlxPublicListingSourceDetailsReaderFactory;
use listing_source_service::{
    ports::{
        PublicListingSourceDetailsReadError, PublicListingSourceDetailsReader,
        PublicListingSourceDetailsReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_find_exact_slug_with_public_projection() {
    let pool = get_postgres_client().await;
    let source_id = uuid::Uuid::now_v7();
    let party_id = uuid::Uuid::now_v7();
    insert_party(&pool, party_id, "detail-operator", "Detail Operator").await;
    insert_source(
        &pool,
        source_id,
        "detail-source",
        "Detail Source",
        party_id,
        Some("https://example.test/source"),
        None,
    )
    .await;

    let result = read(&pool, "detail-source").await;

    assert!(matches!(
        result,
        Some(summary)
            if *summary.listing_source_id.as_uuid() == source_id
                && summary.listing_source_slug_id.as_ref() == "detail-source"
                && summary.name.as_ref() == "Detail Source"
                && summary.operator.name.as_ref() == "Detail Operator"
                && summary.website.as_ref().is_some_and(|website| website.url.as_str() == "https://example.test/source"
                    && website.view_url.as_str() == "https://example.test/source?utm_source=aura_historia&utm_medium=referral")
                && summary.image.is_none()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_route_view_url_through_the_source_referral_configuration() {
    let pool = get_postgres_client().await;
    let party_id = uuid::Uuid::now_v7();
    insert_party(&pool, party_id, "referral-operator", "Referral Operator").await;
    let source_id = uuid::Uuid::now_v7();
    insert_source(
        &pool,
        source_id,
        "referral-source",
        "Referral Source",
        party_id,
        Some("https://example.test/source"),
        None,
    )
    .await;
    set_referral_configuration(
        &pool,
        source_id,
        serde_json::json!({"kind":"PARTNERIZE","camref":"1101l3AbC"}),
    )
    .await;

    let website = read(&pool, "referral-source")
        .await
        .and_then(|summary| summary.website)
        .unwrap_or_else(|| panic!("source should have a website"));

    assert_eq!("https://example.test/source", website.url.as_str());
    assert!(
        website
            .view_url
            .as_str()
            .starts_with("https://prf.hn/click/camref:1101l3AbC/"),
        "{}",
        website.view_url
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_fail_closed_when_the_persisted_referral_configuration_is_invalid() {
    let pool = get_postgres_client().await;
    let party_id = uuid::Uuid::now_v7();
    insert_party(&pool, party_id, "broken-operator", "Broken Operator").await;
    let source_id = uuid::Uuid::now_v7();
    insert_source(
        &pool,
        source_id,
        "broken-source",
        "Broken Source",
        party_id,
        Some("https://example.test/source"),
        None,
    )
    .await;
    set_referral_configuration(&pool, source_id, serde_json::json!({"kind":"OTHER"})).await;

    let result = try_read(&pool, "broken-source").await;

    assert!(matches!(
        result,
        Err(PublicListingSourceDetailsReadError::InvalidReadModel { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_none_for_unknown_or_deleted_exact_slug() {
    let pool = get_postgres_client().await;
    assert!(read(&pool, "unknown-source").await.is_none());

    let source_id = uuid::Uuid::now_v7();
    let party_id = uuid::Uuid::now_v7();
    insert_party(&pool, party_id, "deleted-operator", "Deleted Operator").await;
    insert_source(
        &pool,
        source_id,
        "deleted-source",
        "Deleted Source",
        party_id,
        None,
        None,
    )
    .await;
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
        .bind(source_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("delete test source: {error}"));

    assert!(read(&pool, "deleted-source").await.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_find_existing_ids_ordered_by_id_and_omit_unknown_ids() {
    let pool = get_postgres_client().await;
    let party_id = uuid::Uuid::now_v7();
    insert_party(&pool, party_id, "batch-operator", "Batch Operator").await;
    let first = uuid::Uuid::now_v7();
    let second = uuid::Uuid::now_v7();
    let unrequested = uuid::Uuid::now_v7();
    for (id, slug) in [
        (second, "batch-second"),
        (first, "batch-first"),
        (unrequested, "batch-unrequested"),
    ] {
        insert_source(&pool, id, slug, "Batch Source", party_id, None, None).await;
    }
    let requested = [second, ListingSourceId::new().into_uuid(), first]
        .map(|id| ListingSourceId::try_from(id).unwrap_or_else(|error| panic!("{error}")));

    let summaries = read_ids(&pool, &requested).await;

    assert_eq!(
        vec![first, second],
        summaries
            .iter()
            .map(|summary| *summary.listing_source_id.as_uuid())
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_mark_operator_as_partner_only_while_its_partnership_is_active() {
    let pool = get_postgres_client().await;
    let mut sources = Vec::new();
    for (slug, business_state, partner) in [
        ("partner-active", Some("ACTIVE"), true),
        ("partner-dissolved", Some("DISSOLVED"), false),
        ("partner-none", None, false),
    ] {
        let party_id = uuid::Uuid::now_v7();
        insert_party(&pool, party_id, &format!("{slug}-operator"), slug).await;
        if let Some(business_state) = business_state {
            insert_partnership(&pool, party_id, business_state).await;
        }
        let source_id = uuid::Uuid::now_v7();
        insert_source(&pool, source_id, slug, slug, party_id, None, None).await;
        sources.push((source_id, slug, partner));
    }
    let ids = sources
        .iter()
        .map(|(id, _, _)| ListingSourceId::try_from(*id).unwrap_or_else(|error| panic!("{error}")))
        .collect::<Vec<_>>();

    let by_ids = read_ids(&pool, &ids).await;

    for (source_id, slug, partner) in sources {
        let by_slug = read(&pool, slug)
            .await
            .unwrap_or_else(|| panic!("{slug} should exist"));
        assert_eq!(partner, by_slug.operator.partner, "{slug} by slug");
        let by_id = by_ids
            .iter()
            .find(|summary| *summary.listing_source_id.as_uuid() == source_id)
            .unwrap_or_else(|| panic!("{slug} should be found by ID"));
        assert_eq!(partner, by_id.operator.partner, "{slug} by ID");
    }
}

async fn read_ids(pool: &sqlx::PgPool, ids: &[ListingSourceId]) -> Vec<PublicListingSourceSummary> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin detail reader transaction: {error}"));
    let result = SqlxPublicListingSourceDetailsReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_by_ids(ids)
        .await
        .unwrap_or_else(|error| panic!("read public details by ID: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit detail reader transaction: {error}"));
    result
}

async fn read(pool: &sqlx::PgPool, slug: &str) -> Option<PublicListingSourceSummary> {
    try_read(pool, slug)
        .await
        .unwrap_or_else(|error| panic!("read public detail: {error}"))
}

async fn try_read(
    pool: &sqlx::PgPool,
    slug: &str,
) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin detail reader transaction: {error}"));
    let slug =
        ListingSourceSlugId::raw(slug).unwrap_or_else(|error| panic!("valid test slug: {error}"));
    let result = SqlxPublicListingSourceDetailsReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_by_slug(&slug)
        .await;
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit detail reader transaction: {error}"));
    result
}

async fn insert_partnership(pool: &sqlx::PgPool, party_id: uuid::Uuid, business_state: &str) {
    sqlx::query(
        "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(party_id)
    .bind(business_state)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert test partnership: {error}"));
}

async fn insert_party(pool: &sqlx::PgPool, id: uuid::Uuid, slug: &str, name: &str) {
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert test party: {error}"));
}

async fn insert_source(
    pool: &sqlx::PgPool,
    id: uuid::Uuid,
    slug: &str,
    name: &str,
    party_id: uuid::Uuid,
    url: Option<&str>,
    image: Option<&str>,
) {
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url, image, listing_source_type) VALUES ($1, $2, $3, $4, $5, $6, 'DEALER')")
            .bind(id)
            .bind(slug)
            .bind(name)
            .bind(party_id)
            .bind(url)
            .bind(image)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert test source: {error}"));
}

async fn set_referral_configuration(
    pool: &sqlx::PgPool,
    id: uuid::Uuid,
    referral_configuration: serde_json::Value,
) {
    sqlx::query(
        "UPDATE listing_sources SET referral_configuration = $2 WHERE listing_source_id = $1",
    )
    .bind(id)
    .bind(referral_configuration)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("set test source referral configuration: {error}"));
}
