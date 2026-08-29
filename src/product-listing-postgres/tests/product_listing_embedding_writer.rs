use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use platform_postgres::SqlxUnitOfWork;
const EMBEDDING_DIMENSIONS: usize = 768;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use product_listing_postgres::SqlxProductListingEmbeddingWriterFactory;
use product_listing_service::ports::{
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteOutcome,
    ProductListingEmbeddingWriter, ProductListingEmbeddingWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_embedding_append_enrichment_event_and_advance_revision() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_created_event(&pool).await?;
        let embedding_write = new_write(product_listing_id, source_event_id, EventId::new());
        let outcome = apply(&pool, &embedding_write).await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied, outcome);
        let (embedding, current_event): (Option<Vec<f32>>, uuid::Uuid) = sqlx::query_as(
            "SELECT embedding, event_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(&pool)
        .await?;
        assert_eq!(Some(vec![0.25; EMBEDDING_DIMENSIONS]), embedding);
        assert_eq!(
            uuid::Uuid::from(embedding_write.enrichment_event_id),
            current_event
        );
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE event_id = $1")
                .bind(uuid::Uuid::from(embedding_write.enrichment_event_id))
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            Some(source_event_id.to_string().as_str()),
            payload
                .pointer("/sourceEventId")
                .and_then(serde_json::Value::as_str)
        );
        assert_eq!(
            Some("de"),
            payload
                .pointer("/title/language")
                .and_then(serde_json::Value::as_str)
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "embedding write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_and_stale_without_second_embedding_event() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
    let pool = get_postgres_client().await;
    let (product_listing_id, source_event_id) = insert_product_with_created_event(&pool).await?;
    let embedding_write = new_write(product_listing_id, source_event_id, EventId::new());
    apply(&pool, &embedding_write).await?;
    assert_eq!(
        ProductListingEmbeddingWriteOutcome::Duplicate,
        apply(
            &pool,
            &ProductListingEmbeddingWrite {
                enrichment_event_id: EventId::new(),
                ..embedding_write.clone()
            }
        )
        .await?
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_listing_id)).fetch_one(&pool).await?;
    assert_eq!(1, count);
    let (stale_product_listing_id, stale_event_id) = insert_product_with_created_event(&pool).await?;
    advance_product_revision(&pool, stale_product_listing_id).await?;
    assert_eq!(
        ProductListingEmbeddingWriteOutcome::Stale,
        apply(
            &pool,
            &new_write(stale_product_listing_id, stale_event_id, EventId::new())
        )
        .await?
    );
    Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "embedding duplicate/stale acceptance failed: {result:?}"
    );
}

async fn apply(
    pool: &sqlx::PgPool,
    write: &ProductListingEmbeddingWrite,
) -> Result<ProductListingEmbeddingWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingEmbeddingWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}
fn new_write(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
    enrichment_event_id: EventId,
) -> ProductListingEmbeddingWrite {
    ProductListingEmbeddingWrite {
        product_listing_id,
        source_event_id,
        enrichment_event_id,
        embedding: vec![0.25; EMBEDDING_DIMENSIONS],
        title: Localized::new(Language::De, Title::from("Antiker Stuhl")),
    }
}
async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Embedding party')",
    )
    .bind(party_id)
    .bind(format!("embedding-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Embedding source', $3)").bind(listing_source_id).bind(format!("embedding-source-{listing_source_id}")).bind(party_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, event_id, content_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')").bind(uuid::Uuid::from(product_listing_id)).bind(format!(
                "embedding-product-{}",
                &product_listing_id.to_string()[..6]
            )).bind(uuid::Uuid::from(event_id)).bind(listing_source_id).bind(product_listing_id.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CREATED', 'DOMAIN', '{}', now())").bind(uuid::Uuid::from(event_id)).bind(uuid::Uuid::from(product_listing_id)).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}
async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_AVAILABILITY_CHANGED', 'DOMAIN', '{}', now())").bind(uuid::Uuid::from(event_id)).bind(uuid::Uuid::from(product_listing_id)).execute(&mut *tx).await?;
    sqlx::query("UPDATE product_listings SET event_id = $1 WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
