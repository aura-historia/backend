use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_normalization::normalize_title;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSourceReader, SqlxProductListingDetailsReaderFactory,
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingHistoryReaderFactory, SqlxProductListingRepositoryFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
    SqlxProductListingTranslationSourceReader,
};
use product_listing_service::{
    ports::{
        ProductListingContentAssessmentSourceReader, ProductListingDetailsReadRequest,
        ProductListingDetailsReader, ProductListingDetailsReaderFactory,
        ProductListingEmbeddingSourceReader, ProductListingEventAppender,
        ProductListingEventAppenderFactory, ProductListingHistoryReader,
        ProductListingHistoryReaderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, ProductListingSearchFilterMatchSourceReader,
        ProductListingSearchFilterMatchSourceReaderFactory, ProductListingTranslationSourceReader,
        stamp_product_listing_event,
    },
    use_cases::{
        ProductListingHistoryEntryKind, ProductListingHistoryLookup,
        queries::get_product_listing::ProductListingLookup,
    },
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn normalized_titles_round_trip_through_history_details_and_worker_sources() {
    let pool = get_postgres_client().await;
    for (index, raw) in [
        "a".repeat(200),
        "ä".repeat(200),
        "Sentence title .".to_owned(),
        "Sentence title . .".to_owned(),
        "Existing ellipsis...".to_owned(),
        "Shortened legacy ellipsis..".to_owned(),
    ]
    .into_iter()
    .enumerate()
    {
        let title = normalize_title(&raw).expect("normalize title");
        let (product, event_id) = persist_product(&pool, &format!("title-{index}"), &title).await;
        assert_read_paths(&pool, &product, event_id, &title, &title).await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn legacy_title_reads_preserve_identifiers_versions_and_immutable_event_payloads() {
    let pool = get_postgres_client().await;
    for (slug, current_text, history_text) in [
        (
            "legacy-whitespace",
            "Legacy title ".to_owned(),
            "Legacy title ".to_owned(),
        ),
        (
            "legacy-ellipsis",
            format!("{}..", "A".repeat(125)),
            format!("{}...", "A".repeat(125)),
        ),
    ] {
        let (product, event_id) = persist_product(&pool, slug, &Title::from(&history_text)).await;
        // Fault injection is restricted to this isolated PostgreSQL test fixture.
        sqlx::query("UPDATE product_listings SET title_text = $1 WHERE product_listing_id = $2")
            .bind(&current_text)
            .bind(product.id().into_uuid())
            .execute(&pool)
            .await
            .expect("seed legacy current title");
        sqlx::query("UPDATE product_listing_events SET payload = jsonb_set(payload, '{title,text}', to_jsonb($1::text)) WHERE event_id = $2")
            .bind(&history_text)
            .bind(event_id.into_uuid())
            .execute(&pool)
            .await
            .expect("seed legacy event title");

        let before = stored_title_state(&pool, product.id()).await;
        assert_read_paths(
            &pool,
            &product,
            event_id,
            &Title::from(&current_text),
            &Title::from(&history_text),
        )
        .await;
        assert_eq!(before, stored_title_state(&pool, product.id()).await);
    }
}

async fn assert_read_paths(
    pool: &sqlx::PgPool,
    product: &ProductListing,
    event_id: EventId,
    current_title: &Title,
    history_title: &Title,
) {
    let current_localized = Some(Localized::new(Language::En, current_title.clone()));
    let history_localized = Some(Localized::new(Language::En, history_title.clone()));
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut transaction = unit_of_work.begin().await.expect("begin reads");
    let loaded = SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .find_by_id(product.id())
        .await
        .expect("read aggregate")
        .expect("aggregate exists");
    assert_eq!(current_localized.as_ref(), loaded.value.title());
    assert_eq!(product.title_slug_id(), loaded.value.title_slug_id());

    let details = SqlxProductListingDetailsReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_details(&ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(product.id()),
            language: Language::En,
            user_id: None,
        })
        .await
        .expect("read details")
        .expect("details exist")
        .item;
    assert_eq!(current_localized, details.product_title);
    assert_eq!(current_localized, details.title);
    assert_eq!(product.id(), details.product_listing_id);
    assert_eq!(
        *product.title_slug_id(),
        details.product_listing_title_slug_id
    );
    assert_eq!(event_id, details.event_id);

    let history = SqlxProductListingHistoryReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_history(&ProductListingHistoryLookup::ById(product.id()))
        .await
        .expect("read history")
        .expect("history exists");
    assert_eq!(1, history.len());
    assert_eq!(event_id, history[0].event_id);
    let ProductListingHistoryEntryKind::Discovered(discovery) = &history[0].kind else {
        panic!("expected discovery event");
    };
    assert_eq!(history_localized, discovery.title);

    let match_source = SqlxProductListingSearchFilterMatchSourceReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_source(event_id, product.id())
        .await
        .expect("read percolation source")
        .expect("percolation source exists");
    assert_eq!(current_localized, match_source.product_title);
    assert_eq!(Some(current_title), match_source.titles.get(&Language::En));
    transaction.commit().await.expect("commit reads");

    let assessment_source = SqlxProductListingContentAssessmentSourceReader::new(pool.clone())
        .find_source(event_id, product.id())
        .await
        .expect("read assessment source")
        .expect("assessment source exists");
    assert_eq!(Some(current_title), assessment_source.title.as_ref());
    let embedding_source = SqlxProductListingEmbeddingSourceReader::new(pool.clone())
        .find_source(event_id, product.id())
        .await
        .expect("read embedding source")
        .expect("embedding source exists");
    assert_eq!(current_localized, embedding_source.title);
    let translation_source = SqlxProductListingTranslationSourceReader::new(pool.clone())
        .find_source(event_id, product.id())
        .await
        .expect("read translation source")
        .expect("translation source exists");
    assert_eq!(Some(current_title), translation_source.title.as_ref());
}

async fn persist_product(
    pool: &sqlx::PgPool,
    slug: &str,
    title: &Title,
) -> (ProductListing, EventId) {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(slug)
        .execute(pool)
        .await
        .expect("seed party");
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(format!("{slug}-source"))
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .expect("seed listing source");
    let mut product = ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::raw(&format!("{slug}-a1b2c3"))
            .expect("valid title slug"),
        listing_source_id,
        source_listing_id: SourceListingId::try_from(slug).expect("valid source listing ID"),
        title: Some(Localized::new(Language::En, title.clone())),
        description: None,
        pricing: ProductListingPricing {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: None,
        url: Url::parse(&format!("https://example.com/{slug}")).expect("valid URL"),
        images: IndexSet::new(),
        auction: None,
    })
    .expect("create listing");
    let event = stamp_product_listing_event(
        product.id(),
        OffsetDateTime::now_utc(),
        product
            .take_pending_event_payload()
            .expect("discovery payload"),
    );
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut transaction = unit_of_work.begin().await.expect("begin write");
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(&product, event.event_id)
        .await
        .expect("insert listing");
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&event)
        .await
        .expect("append discovery");
    transaction.commit().await.expect("commit write");
    (product, event.event_id)
}

async fn stored_title_state(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> (String, String, i64, serde_json::Value) {
    sqlx::query_as("SELECT product.title_text, product.product_listing_title_slug_id, product.version, event.payload FROM product_listings product JOIN product_listing_events event ON event.event_id = product.current_event_id WHERE product.product_listing_id = $1")
        .bind(product_listing_id.into_uuid())
        .fetch_one(pool)
        .await
        .expect("snapshot stored title state")
}
