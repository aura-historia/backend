mod support;

use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{
    additional_url::{AdditionalUrl, AdditionalUrls},
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{
        ContentNoticeTarget, ListingSourceSnapshot, ProductListingSnapshot,
        ProductListingTitleSnapshot, TargetSnapshot,
    },
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::{
    SqlxContentNoticeDetailsReaderFactory, SqlxContentNoticeRepositoryFactory,
    SqlxContentNoticeTargetReaderFactory,
};
use content_notice_service::{
    ports::{ContentNoticeDetailsReader, ContentNoticeDetailsReaderFactory},
    use_cases::{
        commands::submit_content_notice::{
            ContentNoticeTargetReference, SubmitContentNoticeCommand, SubmitContentNoticeHandler,
            SubmitContentNoticeUseCase,
        },
        queries::get_admin_content_notice::{
            AdminContentNoticeDetails, CurrentListingSource, CurrentProductListing,
        },
    },
};
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourceSlugId,
};
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use sqlx::PgPool;
use support::{seed_listing_source, seed_product_listing};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn submit(
    pool: &PgPool,
    target: ContentNoticeTargetReference,
    submitter_role: SubmitterRole,
    reason_category: ReasonCategory,
) -> ContentNoticeId {
    SubmitContentNoticeHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxContentNoticeTargetReaderFactory::new(),
        SqlxContentNoticeRepositoryFactory::new(),
    )
    .execute(
        &OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        },
        SubmitContentNoticeCommand {
            target,
            submitter_role,
            reason_category,
            explanation: reason_category
                .requires_explanation()
                .then(|| NoticeExplanation::try_from("Das Foto ist meins.").unwrap()),
            additional_urls: AdditionalUrls::try_from(vec![
                AdditionalUrl::try_from("https://photographer.example/vase").unwrap(),
            ])
            .unwrap(),
            submitter: Submitter {
                name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                email: SubmitterEmail::try_from("erika@example.com").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        },
    )
    .await
    .unwrap()
    .content_notice_id
}

async fn submit_about_source(pool: &PgPool, listing_source_id: ListingSourceId) -> ContentNoticeId {
    submit(
        pool,
        ContentNoticeTargetReference::ListingSource { listing_source_id },
        SubmitterRole::Provider,
        ReasonCategory::ProviderOptOut,
    )
    .await
}

async fn find(pool: &PgPool, id: ContentNoticeId) -> Option<AdminContentNoticeDetails> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let details = SqlxContentNoticeDetailsReaderFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    details
}

async fn execute(pool: &PgPool, sql: &'static str, listing_source_id: ListingSourceId) {
    sqlx::query(sql)
        .bind(listing_source_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn add_ingestion_methods(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    methods: &[ListingIngestionMethod],
) {
    for method in methods {
        sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, $2)")
            .bind(listing_source_id.into_uuid())
            .bind(method.as_str())
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert ingestion method: {error}"));
    }
}

fn slug_of(listing_source_id: ListingSourceId) -> ListingSourceSlugId {
    ListingSourceSlugId::raw(format!("source-{}", listing_source_id.as_uuid().simple())).unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_read_listing_source_notice_with_snapshot_and_current_source() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(
        &pool,
        "Partner Antik",
        Some("https://partner.example/"),
        Some("ACTIVE"),
    )
    .await;
    add_ingestion_methods(
        &pool,
        source,
        &[
            ListingIngestionMethod::WebCrawl,
            ListingIngestionMethod::Shopify,
        ],
    )
    .await;
    let id = submit_about_source(&pool, source).await;

    let details = find(&pool, id).await.expect("stored notice");

    let notice = &details.notice;
    assert_eq!(id, notice.id());
    assert_eq!(ContentNoticeState::Submitted, notice.state());
    assert_eq!(
        ContentNoticeTarget::ListingSource {
            listing_source_id: source
        },
        notice.target()
    );
    assert_eq!(
        &TargetSnapshot {
            listing_source: ListingSourceSnapshot {
                name: ListingSourceName::try_from("Partner Antik").unwrap(),
                url: Some(Url::parse("https://partner.example/").unwrap()),
                partner: true,
            },
            product_listing: None,
        },
        notice.target_snapshot()
    );
    assert_eq!(SubmitterRole::Provider, notice.submitter_role());
    assert_eq!(ReasonCategory::ProviderOptOut, notice.reason_category());
    assert_eq!(None, notice.explanation());
    assert_eq!(1, notice.additional_urls().len());
    assert_eq!("erika@example.com", notice.submitter().email.as_str());
    assert_eq!(1, details.version.into_inner());
    assert_eq!(details.created, details.updated);
    assert_eq!(
        Some(CurrentListingSource {
            name: ListingSourceName::try_from("Partner Antik").unwrap(),
            slug_id: slug_of(source),
            ingestion_methods: vec![
                ListingIngestionMethod::Shopify,
                ListingIngestionMethod::WebCrawl
            ],
            partner: true,
        }),
        details.current_target.listing_source
    );
    assert_eq!(None, details.current_target.product_listing);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_keep_snapshot_and_show_withdrawn_lifecycle_of_a_listing_withdrawn_later() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Plain Antik", None, None).await;
    let listing = seed_product_listing(
        &pool,
        source,
        ListingLifecycle::Active,
        Some("Jugendstil-Vase"),
    )
    .await;
    let id = submit(
        &pool,
        ContentNoticeTargetReference::ProductListing {
            product_listing_id: listing,
        },
        SubmitterRole::RightsHolder,
        ReasonCategory::RightsInfringement,
    )
    .await;
    let active = find(&pool, id).await.expect("stored notice");
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN' WHERE product_listing_id = $1",
    )
    .bind(listing.into_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let withdrawn = find(&pool, id).await.expect("stored notice");

    assert_eq!(
        Some(CurrentProductListing {
            lifecycle: ListingLifecycle::Active
        }),
        active.current_target.product_listing
    );
    assert_eq!(
        Some(CurrentProductListing {
            lifecycle: ListingLifecycle::Withdrawn
        }),
        withdrawn.current_target.product_listing
    );
    assert_eq!(
        ContentNoticeTarget::ProductListing {
            product_listing_id: listing,
            listing_source_id: source,
        },
        withdrawn.notice.target()
    );
    assert_eq!(
        Some(&ProductListingSnapshot {
            title: Some(ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap()),
            url: Url::parse("https://shop.example/vase?id=42").unwrap(),
        }),
        withdrawn.notice.target_snapshot().product_listing.as_ref()
    );
    assert_eq!(
        active.notice.target_snapshot(),
        withdrawn.notice.target_snapshot()
    );
    assert_eq!(
        Some("Das Foto ist meins."),
        withdrawn
            .notice
            .explanation()
            .map(|explanation| explanation.as_str())
    );
    assert!(withdrawn.current_target.listing_source.is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_report_a_deleted_listing_source_as_gone_and_keep_its_snapshot() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Closed Antik", None, None).await;
    add_ingestion_methods(&pool, source, &[ListingIngestionMethod::WebCrawl]).await;
    let id = submit_about_source(&pool, source).await;
    execute(
        &pool,
        "DELETE FROM listing_sources WHERE listing_source_id = $1",
        source,
    )
    .await;

    let details = find(&pool, id).await.expect("notice outlives its target");

    assert_eq!(None, details.current_target.listing_source);
    assert_eq!(None, details.current_target.product_listing);
    assert_eq!(
        "Closed Antik",
        details
            .notice
            .target_snapshot()
            .listing_source
            .name
            .as_ref()
    );
    assert_eq!(source, details.notice.target().listing_source_id());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_show_later_partner_name_and_ingestion_changes_only_in_current_target() {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "Plain Antik", None, None).await;
    add_ingestion_methods(
        &pool,
        source,
        &[
            ListingIngestionMethod::WebCrawl,
            ListingIngestionMethod::Shopify,
        ],
    )
    .await;
    let id = submit_about_source(&pool, source).await;
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id, business_state) SELECT $1, operator_party_id, 'ACTIVE' FROM listing_sources WHERE listing_source_id = $2")
        .bind(uuid::Uuid::now_v7())
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    execute(
        &pool,
        "UPDATE listing_sources SET name = 'Renamed Antik' WHERE listing_source_id = $1",
        source,
    )
    .await;
    execute(
        &pool,
        "DELETE FROM listing_source_ingestion_methods WHERE listing_source_id = $1 AND ingestion_method = 'WEB_CRAWL'",
        source,
    )
    .await;

    let details = find(&pool, id).await.expect("stored notice");

    let snapshot = &details.notice.target_snapshot().listing_source;
    assert!(!snapshot.partner);
    assert_eq!("Plain Antik", snapshot.name.as_ref());
    assert_eq!(
        Some(CurrentListingSource {
            name: ListingSourceName::try_from("Renamed Antik").unwrap(),
            slug_id: slug_of(source),
            ingestion_methods: vec![ListingIngestionMethod::Shopify],
            partner: true,
        }),
        details.current_target.listing_source
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_find_nothing_for_an_unknown_notice() {
    let pool = get_postgres_client().await;

    assert_eq!(None, find(&pool, ContentNoticeId::new()).await);
}
