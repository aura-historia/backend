use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use product_listing_postgres::SqlxProductListingTranslationWriterFactory;
use product_listing_service::ports::{
    ProductListingTranslationWrite, ProductListingTranslationWriteOutcome,
    ProductListingTranslationWriter, ProductListingTranslationWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_all_translations_append_enrichment_event_and_advance_current_event_without_aggregate_version()
 {
    let result: Result<(), Box<dyn std::error::Error>> = async {
    let pool = get_postgres_client().await;
    let (product_listing_id, source_event_id) = insert_product_with_discovered_event(&pool).await?;
    let enrichment_event_id = EventId::new();
    let write = write(product_listing_id, source_event_id, enrichment_event_id);
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;

    let outcome = SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&write)
        .await?;
    tx.commit().await?;

    assert_eq!(ProductListingTranslationWriteOutcome::Applied, outcome);
    let translations = sqlx::query_as::<_, (String, String, uuid::Uuid)>(
        "SELECT language, title, source_event_id FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        vec![
            (
                "en".to_owned(),
                "Antique chair".to_owned(),
                uuid::Uuid::from(source_event_id)
            ),
            (
                "fr".to_owned(),
                "Chaise ancienne".to_owned(),
                uuid::Uuid::from(source_event_id)
            ),
        ],
        translations
    );
    let event = sqlx::query_as::<_, (String, String, serde_json::Value)>(
        "SELECT event_type, event_group, payload FROM product_listing_events WHERE event_id = $1",
    )
    .bind(uuid::Uuid::from(enrichment_event_id))
    .fetch_one(&pool)
    .await?;
    assert_eq!("ENRICHMENT_TRANSLATED_TITLES", event.0);
    assert_eq!("ENRICHMENT", event.1);
    assert_eq!(
        Some("de"),
        event
            .2
            .pointer("/sourceLanguage")
            .and_then(serde_json::Value::as_str)
    );
    assert_eq!(
        Some(&vec![serde_json::Value::String("en".to_owned()), serde_json::Value::String("fr".to_owned())]),
        event.2.pointer("/targetLanguages").and_then(serde_json::Value::as_array)
    );
    assert!(event.2.pointer("/titles").is_none());
    let (current_event, version, projection_version): (uuid::Uuid, i64, i64) =
        sqlx::query_as("SELECT current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1")
            .bind(uuid::Uuid::from(product_listing_id))
            .fetch_one(&pool)
            .await?;
    assert_eq!(uuid::Uuid::from(enrichment_event_id), current_event);
    assert_eq!(1, version);
    assert_eq!(2, projection_version);
    Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "translation write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_without_second_event_when_same_source_is_redelivered() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) = insert_product_with_discovered_event(&pool).await?;
        let write = write(product_listing_id, source_event_id, EventId::new());
        apply(&pool, &write).await?;

        let outcome = apply(
            &pool,
            &ProductListingTranslationWrite {
                enrichment_event_id: EventId::new(),
                ..write
            },
        )
        .await?;

        assert_eq!(ProductListingTranslationWriteOutcome::Duplicate, outcome);
        let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_group = 'ENRICHMENT'",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await?;
        assert_eq!(1, count, "one translated event for the discovered source");
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation duplicate acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_stale_without_writing_when_content_source_event_advanced() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_discovered_event(&pool).await?;
        let newer_event_id = insert_event_and_advance_product(
            &pool,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
        )
        .await?;

        let outcome = apply(
            &pool,
            &write(product_listing_id, source_event_id, EventId::new()),
        )
        .await?;

        assert_eq!(ProductListingTranslationWriteOutcome::Stale, outcome);
        let translation_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM product_listing_translations WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, translation_count);
        let current_event = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(&pool)
        .await?;
        assert_eq!(uuid::Uuid::from(newer_event_id), current_event);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation stale acceptance failed: {result:?}"
    );
}

async fn apply(
    pool: &sqlx::PgPool,
    write: &ProductListingTranslationWrite,
) -> Result<ProductListingTranslationWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

fn write(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
    enrichment_event_id: EventId,
) -> ProductListingTranslationWrite {
    ProductListingTranslationWrite {
        product_listing_id,
        source_event_id,
        enrichment_event_id,
        source_language: Language::De,
        titles: IndexMap::from([
            (Language::En, Title::from("Antique chair")),
            (Language::Fr, Title::from("Chaise ancienne")),
        ]),
    }
}

async fn insert_product_with_discovered_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Translation party')",
    )
    .bind(party_id)
    .bind(format!("translation-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Translation source', $3)")
        .bind(listing_source_id)
        .bind(format!("translation-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(title_slug("translation-product", product_listing_id))
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

async fn insert_event_and_advance_product(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

fn title_slug(prefix: &str, product_listing_id: ProductListingId) -> String {
    format!("{prefix}-{}", &product_listing_id.to_string()[..6])
}
