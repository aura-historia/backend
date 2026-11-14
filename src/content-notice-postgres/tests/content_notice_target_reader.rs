use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{
    additional_url::AdditionalUrls,
    content_notice_target::{ContentNoticeTarget, ListingSourceSnapshot},
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::{
    SqlxContentNoticeRepositoryFactory, SqlxContentNoticeTargetReaderFactory,
};
use content_notice_service::{
    ports::{
        ContentNoticeRepository, ContentNoticeRepositoryFactory, ContentNoticeTargetReader,
        ContentNoticeTargetReaderFactory, ListingSourceTarget, ProductListingTarget,
    },
    use_cases::commands::submit_content_notice::{
        ContentNoticeTargetReference, SubmitContentNoticeCommand, SubmitContentNoticeHandler,
        SubmitContentNoticeUseCase,
    },
};
use listing_source_core::{ListingSourceId, ListingSourceName};
use localization::Language;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn begin(pool: &PgPool) -> SqlxTransaction {
    SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin target read transaction: {error}"))
}

/// Seeds a listing source whose operator holds a partnership in `partnership_state`, if any.
async fn seed_listing_source(
    pool: &PgPool,
    name: &str,
    url: Option<&str>,
    partnership_state: Option<&str>,
) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("party-{}", party_id.simple()))
        .bind(format!("{name} operator"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert party: {error}"));
    if let Some(state) = partnership_state {
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(party_id)
        .bind(state)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert partnership: {error}"));
    }
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url, listing_source_type) VALUES ($1, $2, $3, $4, $5, 'DEALER')")
        .bind(listing_source_id.into_uuid())
        .bind(format!("source-{}", listing_source_id.as_uuid().simple()))
        .bind(name)
        .bind(party_id)
        .bind(url)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
    listing_source_id
}

async fn seed_product_listing(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    lifecycle: ListingLifecycle,
    title: Option<&str>,
) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let event_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8, 'https://shop.example/vase?id=42')")
        .bind(product_listing_id.into_uuid())
        .bind(format!("vase-{}", &product_listing_id.as_uuid().simple().to_string()[26..]))
        .bind(event_id)
        .bind(listing_source_id.into_uuid())
        .bind(product_listing_id.to_string())
        .bind(title)
        .bind(title.map(|_| "de"))
        .bind(lifecycle.as_str())
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("insert product listing: {error}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(event_id)
        .bind(product_listing_id.into_uuid())
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("insert product listing event: {error}"));
    tx.commit().await.unwrap();
    product_listing_id
}

async fn find_listing_source(pool: &PgPool, id: ListingSourceId) -> Option<ListingSourceTarget> {
    let mut tx = begin(pool).await;
    SqlxContentNoticeTargetReaderFactory::new()
        .in_transaction(&mut tx)
        .find_listing_source(id)
        .await
        .unwrap()
}

async fn find_product_listing(pool: &PgPool, id: ProductListingId) -> Option<ProductListingTarget> {
    let mut tx = begin(pool).await;
    SqlxContentNoticeTargetReaderFactory::new()
        .in_transaction(&mut tx)
        .find_product_listing(id)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_snapshot_listing_source_with_partner_flag_of_its_operator() {
    let pool = get_postgres_client().await;
    let partner = seed_listing_source(
        &pool,
        "Partner Antik",
        Some("https://partner.example/"),
        Some("ACTIVE"),
    )
    .await;
    let dissolved = seed_listing_source(&pool, "Former Partner", None, Some("DISSOLVED")).await;
    let plain = seed_listing_source(&pool, "Plain Antik", None, None).await;

    assert_eq!(
        Some(ListingSourceTarget {
            listing_source_id: partner,
            snapshot: ListingSourceSnapshot {
                name: ListingSourceName::try_from("Partner Antik").unwrap(),
                url: Some(Url::parse("https://partner.example/").unwrap()),
                partner: true,
            },
        }),
        find_listing_source(&pool, partner).await
    );
    for id in [dissolved, plain] {
        let target = find_listing_source(&pool, id).await.unwrap();
        assert!(!target.snapshot.partner);
        assert_eq!(None, target.snapshot.url);
    }
    assert_eq!(
        None,
        find_listing_source(&pool, ListingSourceId::new()).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_snapshot_product_listing_with_its_lifecycle_and_listing_source() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Partner Antik", None, Some("ACTIVE")).await;
    let active = seed_product_listing(
        &pool,
        source,
        ListingLifecycle::Active,
        Some("Jugendstil-Vase"),
    )
    .await;
    let withdrawn = seed_product_listing(&pool, source, ListingLifecycle::Withdrawn, None).await;

    let active_target = find_product_listing(&pool, active).await.unwrap();
    let withdrawn_target = find_product_listing(&pool, withdrawn).await.unwrap();

    assert_eq!(active, active_target.product_listing_id);
    assert_eq!(ListingLifecycle::Active, active_target.lifecycle);
    assert_eq!(
        Some("Jugendstil-Vase"),
        active_target
            .snapshot
            .title
            .as_ref()
            .map(|title| title.as_str())
    );
    assert_eq!(
        "https://shop.example/vase?id=42",
        active_target.snapshot.url.as_str()
    );
    assert_eq!(source, active_target.listing_source.listing_source_id);
    assert!(active_target.listing_source.snapshot.partner);
    assert_eq!(ListingLifecycle::Withdrawn, withdrawn_target.lifecycle);
    assert_eq!(None, withdrawn_target.snapshot.title);
    assert_eq!(
        None,
        find_product_listing(&pool, ProductListingId::new()).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_commit_submitted_notice_with_resolved_target() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Plain Antik", None, None).await;
    let listing = seed_product_listing(
        &pool,
        source,
        ListingLifecycle::Active,
        Some("Jugendstil-Vase"),
    )
    .await;
    let handler = SubmitContentNoticeHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxContentNoticeTargetReaderFactory::new(),
        SqlxContentNoticeRepositoryFactory::new(),
    );

    let result = handler
        .execute(
            &OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("request"),
                correlation_id: CorrelationId::new("correlation"),
            },
            SubmitContentNoticeCommand {
                target: ContentNoticeTargetReference::ProductListing {
                    product_listing_id: listing,
                },
                submitter_role: SubmitterRole::RightsHolder,
                reason_category: ReasonCategory::RightsInfringement,
                explanation: Some(NoticeExplanation::try_from("Das Foto ist meins.").unwrap()),
                additional_urls: AdditionalUrls::default(),
                submitter: Submitter {
                    name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                    email: SubmitterEmail::try_from("erika@example.com").unwrap(),
                    language: Language::De,
                },
                good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
            },
        )
        .await
        .unwrap();

    let mut tx = begin(&pool).await;
    let stored = SqlxContentNoticeRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(result.content_notice_id)
        .await
        .unwrap()
        .expect("committed notice")
        .value;
    tx.commit().await.unwrap();
    assert_eq!(
        ContentNoticeTarget::ProductListing {
            product_listing_id: listing,
            listing_source_id: source,
        },
        stored.target()
    );
    assert!(!stored.target_snapshot().listing_source.partner);
    assert_eq!(
        Some("Jugendstil-Vase"),
        stored
            .target_snapshot()
            .product_listing
            .as_ref()
            .and_then(|listing| listing.title.as_ref())
            .map(|title| title.as_str())
    );
}
