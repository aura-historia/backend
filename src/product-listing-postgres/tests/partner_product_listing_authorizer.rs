use application::transaction::UnitOfWork;
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxPartnerProductListingAuthorizerFactory;
use product_listing_service::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_authorize_admin_for_existing_listing_source() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductListingAuthorizerFactory::new();
    let admin_id = seed_user(&pool, "ADMIN").await;
    let listing_source_id = seed_listing_source(&pool, "admin-product-authorizer-source").await;

    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin authorization transaction: {error}"));
    let result = authorizer
        .in_transaction(&mut tx)
        .authorize(admin_id, listing_source_id)
        .await;

    assert!(matches!(result, Ok(())));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_authorize_partnership_member_or_grantee_and_reject_unrelated_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductListingAuthorizerFactory::new();
    let member_id = seed_user(&pool, "USER").await;
    let grantee_id = seed_user(&pool, "USER").await;
    let unrelated_user_id = seed_user(&pool, "USER").await;
    let (listing_source_id, partnership_id) =
        seed_partnership_listing_source(&pool, "partner-product-authorizer-source").await;

    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(uuid::Uuid::from(member_id))
        .bind(partnership_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("seed partnership membership: {error}"));
    sqlx::query(
        "INSERT INTO partnership_listing_source_grants (user_id, listing_source_id) VALUES ($1, $2)",
    )
    .bind(uuid::Uuid::from(grantee_id))
    .bind(uuid::Uuid::from(listing_source_id))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed listing-source grant: {error}"));

    for actor_id in [member_id, grantee_id] {
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin authorization transaction: {error}"));
        let result = authorizer
            .in_transaction(&mut tx)
            .authorize(actor_id, listing_source_id)
            .await;
        assert!(matches!(result, Ok(())));
    }

    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin authorization transaction: {error}"));
    let result = authorizer
        .in_transaction(&mut tx)
        .authorize(unrelated_user_id, listing_source_id)
        .await;
    assert!(matches!(
        result,
        Err(PartnerProductListingAuthorizationError::Forbidden)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_missing_listing_source() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let authorizer = SqlxPartnerProductListingAuthorizerFactory::new();
    let user_id = seed_user(&pool, "USER").await;

    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin authorization transaction: {error}"));
    let result = authorizer
        .in_transaction(&mut tx)
        .authorize(user_id, ListingSourceId::new())
        .await;
    assert!(matches!(
        result,
        Err(PartnerProductListingAuthorizationError::ListingSourceNotFound)
    ));
}

async fn seed_user(pool: &sqlx::PgPool, role: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', $3)")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@example.test"))
        .bind(role)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed user: {error}"));
    user_id
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed source party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(slug)
    .bind(slug)
    .bind(party_id)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}

async fn seed_partnership_listing_source(
    pool: &sqlx::PgPool,
    slug: &str,
) -> (ListingSourceId, uuid::Uuid) {
    let party_id = uuid::Uuid::new_v4();
    let partnership_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed partnership party: {error}"));
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed partnership: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(slug)
    .bind(slug)
    .bind(party_id)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    (listing_source_id, partnership_id)
}
