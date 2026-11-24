//! The live schema audit's persistence: which ListingSources are due, what it samples, the review
//! it opens and the findings an approval accepted. The audit's judgement is unit-tested in
//! `review::schema_audit`; the hold on new listings in `tests/candidates.rs`.

use crawler::review::model::{PAGE_ROLE_SEED, STATUS_PENDING_REVIEW, SchemaReviewPageInput};
use crawler::review::repository::{
    CrawlerReviewRepository, SchemaAuditOpening, SchemaAuditReview, SchemaAuditSource,
};
use crawler::review::schema_audit::SCHEMA_AUDIT_REASON;
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::time::Duration;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn schema(title_selector: &str) -> ProductCssSelectorSchema {
    let rule = |selector: &str| ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    };
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: Some(rule(title_selector)),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: Some(rule("span.state")),
        images: Some(ExtractionRule {
            extract: ExtractionKind::ImageUrl,
            cardinality: ExtractionCardinality::All,
            ..rule("img")
        }),
        raw_attributes: Default::default(),
        sanitize: Default::default(),
    }
}

/// A crawl-enabled ListingSource named `name` on `host`, with `live_schemas` stored unless empty.
async fn insert_source(
    pool: &PgPool,
    name: &str,
    host: &str,
    live_schemas: &[ProductCssSelectorSchema],
) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, $2, $3, TRUE, $4, $3, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(name)
    .bind(host)
    .bind(format!("https://{host}/"))
    .execute(pool)
    .await
    .unwrap();
    if !live_schemas.is_empty() {
        sqlx::query(
            "INSERT INTO listing_source_product_schemas \
             (listing_source_id, product_schema, created, updated) VALUES ($1, $2, NOW(), NOW())",
        )
        .bind(listing_source_id.as_uuid())
        .bind(serde_json::to_value(live_schemas).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    listing_source_id
}

async fn set_audited_days_ago(pool: &PgPool, listing_source_id: ListingSourceId, days: i32) {
    sqlx::query(
        "UPDATE listing_sources SET schema_audited_at = NOW() - make_interval(days => $2) \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(days)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_product_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    url: &str,
    captured_identity: Option<&str>,
) {
    UrlMetadataRepositoryImpl::new(pool.clone())
        .upsert_link(
            &listing_source_id,
            &url::Url::parse(url).unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE listing_source_urls SET last_source_listing_id = $2 WHERE url = $1")
        .bind(url)
        .bind(captured_identity)
        .execute(pool)
        .await
        .unwrap();
}

fn audited_pages(host: &str) -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: format!("https://{host}/p/1"),
        role: PAGE_ROLE_SEED.to_owned(),
        raw_html:
            "<html><head><title>Chair | Shop</title></head><body><h1>Chair</h1></body></html>"
                .to_owned(),
    }]
}

fn findings() -> Vec<String> {
    vec![
        "title LISTING_SOURCE_NAME".to_owned(),
        "title READS_DOCUMENT_TITLE".to_owned(),
    ]
}

/// Decides the review as `actor`, as the review's trail records a verdict, `reviewed` now.
async fn decide(pool: &PgPool, review_id: crawler::CrawlerReviewId, status: &str, actor: &str) {
    sqlx::query(
        "UPDATE crawler_reviews SET status = $2, reviewed = NOW(), updated = NOW() \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_events (event_id, review_id, actor, action, candidate_version, details) \
         VALUES ($1, $2, $3, $4, 1, '{}'::jsonb)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(review_id.as_uuid())
    .bind(actor)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

/// The review the audit opened.
async fn open(
    repository: &CrawlerReviewRepository,
    source: &ListingSourceId,
    live: &[ProductCssSelectorSchema],
    host: &str,
) -> crawler::CrawlerReviewId {
    match repository
        .open_schema_audit_review(
            source,
            live,
            audited_pages(host),
            SchemaAuditOpening {
                findings: &findings(),
                request_note: None,
                holds_new_listings: true,
            },
        )
        .await
        .unwrap()
    {
        SchemaAuditReview::Opened(review_id) => review_id,
        SchemaAuditReview::Deferred => panic!("the audit must open its own review"),
    }
}

fn due_source(listing_source_id: ListingSourceId, name: &str) -> SchemaAuditSource {
    SchemaAuditSource {
        listing_source_id,
        listing_source_name: name.to_owned(),
        request_note: None,
    }
}

/// Sources never audited come first, then the longest ago. Not due are: a source audited within
/// the audit age, one without live schemas, one with a pending schema review, one not crawled, one
/// whose scrape circuit cools down, and one whose audit review was decided within the audit age,
/// however long ago it was opened. An audit it records moves the source out of the due set.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_sources_never_audited_first_and_skip_those_not_due() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("h1")];
    let never = insert_source(&pool, "Never", "never.example", &live).await;
    let long_ago = insert_source(&pool, "Long ago", "long-ago.example", &live).await;
    set_audited_days_ago(&pool, long_ago, 8).await;
    let recent = insert_source(&pool, "Recent", "recent.example", &live).await;
    set_audited_days_ago(&pool, recent, 1).await;
    insert_source(&pool, "No schemas", "no-schemas.example", &[]).await;
    let pending = insert_source(&pool, "Pending", "pending.example", &live).await;
    open(&repository, &pending, &live, "pending.example").await;
    let disabled = insert_source(&pool, "Disabled", "disabled.example", &live).await;
    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(disabled.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let cooling = insert_source(&pool, "Cooling", "cooling.example", &live).await;
    sqlx::query(
        "UPDATE listing_sources SET scrape_failure_streak = 3, \
         next_scrape_at = NOW() + INTERVAL '1 hour' WHERE listing_source_id = $1",
    )
    .bind(cooling.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let rejected = insert_source(&pool, "Rejected", "rejected.example", &live).await;
    let review_id = open(&repository, &rejected, &live, "rejected.example").await;
    set_audited_days_ago(&pool, rejected, 8).await;
    decide(&pool, review_id, "REJECTED", "HUMAN").await;

    let due = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();
    repository.record_schema_audit(&never).await.unwrap();
    let due_after_audit = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();

    assert_eq!(
        due,
        vec![due_source(never, "Never"), due_source(long_ago, "Long ago")]
    );
    assert_eq!(due_after_audit, vec![due_source(long_ago, "Long ago")]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_sample_the_live_schemas_and_only_captured_product_urls() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    insert_product_url(&pool, source, "https://shop.example/p/1", Some("SKU-1")).await;
    insert_product_url(&pool, source, "https://shop.example/p/new", None).await;

    let (schemas, urls) = repository.schema_audit_sample(&source).await.unwrap();

    assert_eq!(schemas, live);
    assert_eq!(
        urls.iter()
            .map(|page| (page.url.as_str(), page.captured_identity.as_str()))
            .collect::<Vec<_>>(),
        vec![("https://shop.example/p/1", "SKU-1")]
    );
}

/// The review's candidate is the live schemas as they stand, and it records the findings, which
/// the review tools show the agents.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_pending_review_of_the_live_schemas_that_records_the_findings() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;

    let review_id = open(&repository, &source, &live, "shop.example").await;

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_PENDING_REVIEW);
    assert_eq!(detail.review.reason, SCHEMA_AUDIT_REASON);
    assert_eq!(
        detail.review.candidate_payload["schemas"],
        serde_json::to_value(live).unwrap()
    );
    assert_eq!(
        detail.review.validation_summary["schema_audit"]["findings"],
        serde_json::json!(findings())
    );
    assert_eq!(
        detail
            .pages
            .iter()
            .map(|page| page.role.as_str())
            .collect::<Vec<_>>(),
        vec![PAGE_ROLE_SEED]
    );
    assert_eq!(
        repository.pending_schema_audit_reviews(WEEK).await.unwrap(),
        1
    );
    assert_eq!(
        repository
            .pending_schema_audit_reviews(Duration::ZERO)
            .await
            .unwrap(),
        0,
        "a review older than the audit age waits for a human and no longer counts"
    );
}

/// A schema review the scraper opened since the audit took the source holds it: the audit opens
/// none, adds nothing to that review, and forgets its record so the source is audited again once
/// that review is decided.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_defer_to_a_schema_review_the_scraper_opened_meanwhile() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    repository.record_schema_audit(&source).await.unwrap();
    let scraper_review = repository
        .create_schema_review(
            &source,
            "manual_schema_required",
            &live,
            audited_pages("shop.example"),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let opened = repository
        .open_schema_audit_review(
            &source,
            &live,
            audited_pages("shop.example"),
            SchemaAuditOpening {
                findings: &findings(),
                request_note: None,
                holds_new_listings: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(opened, SchemaAuditReview::Deferred);
    let detail = repository.get_review(scraper_review).await.unwrap();
    assert_eq!(detail.review.reason, "manual_schema_required");
    assert_eq!(detail.pages.len(), 1);
    assert_eq!(
        repository.pending_schema_audit_reviews(WEEK).await.unwrap(),
        0
    );
    decide(&pool, scraper_review, "APPROVED", "HUMAN").await;
    assert_eq!(
        repository.schema_audit_due_sources(WEEK, 10).await.unwrap(),
        vec![due_source(source, "Shop")]
    );
}

/// Every audit review that judged exactly the live schemas accepted its findings: an approval by
/// anyone, or a human's rejection. A pending review, an agent's rejection, or live schemas changed
/// since accepted nothing, and samples that differ between audits are covered by every such
/// review, not only the last one.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_accept_the_findings_of_every_audit_review_that_judged_the_same_live_schemas() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    let accepted = |live: Vec<ProductCssSelectorSchema>| {
        let repository = repository.clone();
        async move {
            let mut findings = repository
                .accepted_schema_audit_findings(&source, &live)
                .await
                .unwrap();
            findings.sort();
            findings
        }
    };
    let first = open(&repository, &source, &live, "shop.example").await;

    let while_pending = accepted(live.to_vec()).await;
    decide(&pool, first, "APPROVED", "AGENT_REVIEWER").await;
    let after_approval = accepted(live.to_vec()).await;
    let other_schemas = accepted(vec![schema("h1")]).await;
    let second = match repository
        .open_schema_audit_review(
            &source,
            &live,
            audited_pages("shop.example"),
            SchemaAuditOpening {
                findings: &["images NO_IMAGE_ON_ANY_PAGE".to_owned()],
                request_note: None,
                holds_new_listings: true,
            },
        )
        .await
        .unwrap()
    {
        SchemaAuditReview::Opened(review_id) => review_id,
        SchemaAuditReview::Deferred => panic!("the audit must open its own review"),
    };
    decide(&pool, second, "REJECTED", "AGENT_REVIEWER").await;
    let after_agent_rejection = accepted(live.to_vec()).await;
    let third = match repository
        .open_schema_audit_review(
            &source,
            &live,
            audited_pages("shop.example"),
            SchemaAuditOpening {
                findings: &["price TOO_LONG".to_owned()],
                request_note: None,
                holds_new_listings: true,
            },
        )
        .await
        .unwrap()
    {
        SchemaAuditReview::Opened(review_id) => review_id,
        SchemaAuditReview::Deferred => panic!("the audit must open its own review"),
    };
    decide(&pool, third, "REJECTED", "HUMAN").await;
    let after_human_rejection = accepted(live.to_vec()).await;

    assert_eq!(while_pending, Vec::<String>::new());
    assert_eq!(after_approval, findings());
    assert_eq!(other_schemas, Vec::<String>::new());
    assert_eq!(after_agent_rejection, findings());
    assert_eq!(
        after_human_rejection,
        vec![
            "price TOO_LONG".to_owned(),
            "title LISTING_SOURCE_NAME".to_owned(),
            "title READS_DOCUMENT_TITLE".to_owned(),
        ]
    );
}

/// A human's request puts the source ahead of the others even right after an audit and a decided
/// audit review, carries the note into the review it opens, and is settled by that review.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_audit_a_requested_source_first_and_carry_the_note_into_its_review() {
    use crawler::review::repository::SchemaAuditRequestNote;

    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("h1")];
    let never = insert_source(&pool, "Never", "never.example", &live).await;
    let requested = insert_source(&pool, "Ronald Phillips", "phillips.example", &live).await;
    let earlier = open(&repository, &requested, &live, "phillips.example").await;
    decide(&pool, earlier, "APPROVED", "AGENT_REVIEWER").await;
    repository.record_schema_audit(&requested).await.unwrap();
    let note = SchemaAuditRequestNote::parse(
        "Titles dropped the dating and origin the page shows; keep them or capture them in rawPeriod.",
    )
    .unwrap();

    repository
        .request_schema_audit(&requested, &note)
        .await
        .unwrap();
    let due = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();
    let review = repository
        .open_schema_audit_review(
            &requested,
            &live,
            audited_pages("phillips.example"),
            SchemaAuditOpening {
                findings: &[],
                request_note: Some(note.as_str()),
                holds_new_listings: false,
            },
        )
        .await
        .unwrap();
    let missing = repository
        .request_schema_audit(&ListingSourceId::new(), &note)
        .await;

    assert_eq!(
        due,
        vec![
            SchemaAuditSource {
                listing_source_id: requested,
                listing_source_name: "Ronald Phillips".to_owned(),
                request_note: Some(note.as_str().to_owned()),
            },
            due_source(never, "Never"),
        ]
    );
    let SchemaAuditReview::Opened(review_id) = review else {
        panic!("the audit must open its own review");
    };
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.validation_summary["schema_audit"]["request_note"],
        serde_json::json!(note.as_str())
    );
    assert!(missing.is_err(), "a source that does not exist is refused");
    assert_eq!(
        repository.schema_audit_due_sources(WEEK, 10).await.unwrap(),
        vec![due_source(never, "Never")],
        "its pending review holds the requested source"
    );
    decide(&pool, review_id, "APPROVED", "AGENT_REVIEWER").await;
    assert_eq!(
        repository.schema_audit_due_sources(WEEK, 10).await.unwrap(),
        vec![due_source(never, "Never")],
        "the approval of the review that carries the note settles the request"
    );
}

/// A request settles only through a review that carries its note: one that arrives while the
/// source is being audited outlives the review that audit opens, and an agent's rejection of a
/// requested review settles nothing, so the request is due again a day after its last audit.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_a_request_no_decided_review_with_its_note_settled() {
    use crawler::review::repository::SchemaAuditRequestNote;

    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("h1")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    let requested = |note: &'static str| SchemaAuditSource {
        listing_source_id: source,
        listing_source_name: "Shop".to_owned(),
        request_note: Some(note.to_owned()),
    };
    repository.record_schema_audit(&source).await.unwrap();
    let during_the_audit = SchemaAuditRequestNote::parse("Keep the dating.").unwrap();
    repository
        .request_schema_audit(&source, &during_the_audit)
        .await
        .unwrap();
    let unrequested = open(&repository, &source, &live, "shop.example").await;
    decide(&pool, unrequested, "APPROVED", "AGENT_REVIEWER").await;

    let after_unrequested_review = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();
    repository.record_schema_audit(&source).await.unwrap();
    let requested_review = match repository
        .open_schema_audit_review(
            &source,
            &live,
            audited_pages("shop.example"),
            SchemaAuditOpening {
                findings: &[],
                request_note: Some(during_the_audit.as_str()),
                holds_new_listings: false,
            },
        )
        .await
        .unwrap()
    {
        SchemaAuditReview::Opened(review_id) => review_id,
        SchemaAuditReview::Deferred => panic!("the audit must open its own review"),
    };
    decide(&pool, requested_review, "REJECTED", "AGENT_REVIEWER").await;
    let right_after_agent_rejection = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();
    set_audited_days_ago(&pool, source, 2).await;
    let a_day_later = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();

    assert_eq!(
        after_unrequested_review,
        vec![requested("Keep the dating.")]
    );
    assert_eq!(right_after_agent_rejection, vec![]);
    assert_eq!(a_day_later, vec![requested("Keep the dating.")]);
}

/// A pass audits each due source on its captured pages as the shared fetcher serves them: a title
/// rule on the document `<title>` that adds the shop's name opens an audit review, a rule on the
/// heading only records the audit, and so does the same bad rule on a shop whose captured pages
/// now redirect to its home page, which shows no product to audit.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_review_only_for_the_source_whose_live_schemas_fail_the_audit() {
    use crawler::scraper::scraper_service::{FetchedHtml, MockHtmlFetcher};
    use crawler::service::cron::CronStop;
    use crawler::service::schema_audit::SchemaAudit;
    use std::sync::Arc;

    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let failing = insert_source(
        &pool,
        "Bauman Rare Books",
        "bauman.example",
        &[schema("title")],
    )
    .await;
    let passing = insert_source(&pool, "Heading Shop", "heading.example", &[schema("h1")]).await;
    let moved = insert_source(&pool, "Moved Books", "moved.example", &[schema("title")]).await;
    for (source, host) in [
        (failing, "bauman.example"),
        (passing, "heading.example"),
        (moved, "moved.example"),
    ] {
        for item in ["suttree", "human-condition"] {
            insert_product_url(
                &pool,
                source,
                &format!("https://{host}/p/{item}"),
                Some(item),
            )
            .await;
        }
    }
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().returning(|url| {
        let item = url.path().trim_start_matches("/p/").to_owned();
        let html = format!(
            "<html><head><title>{item} First Edition - Bauman Rare Books</title></head><body>\
             <h1>{item}</h1><span class=\"id\">{item}</span><span class=\"state\">In stock</span>\
             <img src=\"/images/{item}.jpg\"></body></html>"
        );
        let final_url = if url.host_str() == Some("moved.example") {
            url.join("/").unwrap()
        } else {
            url.clone()
        };
        Box::pin(async move { Ok(FetchedHtml::new(html, final_url, reqwest::StatusCode::OK)) })
    });
    let audit = SchemaAudit::new(repository.clone(), Arc::new(fetcher));

    let requested = insert_source(
        &pool,
        "Ronald Phillips",
        "phillips.example",
        &[schema("h1")],
    )
    .await;
    for item in ["cabinet", "table"] {
        insert_product_url(
            &pool,
            requested,
            &format!("https://phillips.example/p/{item}"),
            Some(item),
        )
        .await;
    }
    repository
        .request_schema_audit(
            &requested,
            &crawler::review::repository::SchemaAuditRequestNote::parse(
                "Titles dropped the dating the page shows.",
            )
            .unwrap(),
        )
        .await
        .unwrap();

    audit.run_once(&CronStop::default()).await;

    let reasons: Vec<(uuid::Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT listing_source_id, reason, validation_summary->'schema_audit'->'findings' \
         FROM crawler_reviews WHERE validation_summary->'schema_audit'->'request_note' = 'null'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        reasons,
        vec![(
            *failing.as_uuid(),
            SCHEMA_AUDIT_REASON.to_owned(),
            serde_json::json!(["title LISTING_SOURCE_NAME", "title READS_DOCUMENT_TITLE"])
        )]
    );
    let requested_review: (String, serde_json::Value) = sqlx::query_as(
        "SELECT validation_summary->'schema_audit'->>'request_note', \
                validation_summary->'schema_audit'->'findings' \
         FROM crawler_reviews WHERE listing_source_id = $1",
    )
    .bind(requested.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        requested_review,
        (
            "Titles dropped the dating the page shows.".to_owned(),
            serde_json::json!([])
        ),
        "a requested audit opens its review though the live schemas pass every check"
    );
    assert_eq!(
        repository.schema_audit_due_sources(WEEK, 10).await.unwrap(),
        vec![],
        "every source is audited"
    );
}
