use application::transaction::{Transaction, UnitOfWork};
use common::{event_id::EventId, language::domain::Language, product_id::ProductId};
use indexmap::IndexMap;
use platform_postgres::SqlxUnitOfWork;
use product_core::title::Title;
use product_postgres::SqlxProductTranslationWriterFactory;
use product_service::ports::{
    ProductTranslationWrite, ProductTranslationWriteOutcome, ProductTranslationWriter,
    ProductTranslationWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_all_translations_append_enrichment_event_and_advance_product_revision() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
    let pool = get_postgres_client().await;
    let (product_id, source_event_id) = insert_product_with_embedded_event(&pool).await?;
    let enrichment_event_id = EventId::new();
    let write = write(product_id, source_event_id, enrichment_event_id);
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;

    let outcome = SqlxProductTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&write)
        .await?;
    tx.commit().await?;

    assert_eq!(ProductTranslationWriteOutcome::Applied, outcome);
    let translations = sqlx::query_as::<_, (String, String, uuid::Uuid)>(
        "SELECT language, title, source_event_id FROM product_translations WHERE product_id = $1 ORDER BY language",
    )
    .bind(uuid::Uuid::from(product_id))
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
        "SELECT event_type, event_group, payload FROM product_events WHERE event_id = $1",
    )
    .bind(uuid::Uuid::from(enrichment_event_id))
    .fetch_one(&pool)
    .await?;
    assert_eq!("ENRICHMENT_TRANSLATED_TITLES", event.0);
    assert_eq!("ENRICHMENT", event.1);
    assert_eq!(
        Some("Antique chair"),
        event
            .2
            .pointer("/titles/en")
            .and_then(serde_json::Value::as_str)
    );
    let current_event =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT event_id FROM products WHERE product_id = $1")
            .bind(uuid::Uuid::from(product_id))
            .fetch_one(&pool)
            .await?;
    assert_eq!(uuid::Uuid::from(enrichment_event_id), current_event);
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
        let (product_id, source_event_id) = insert_product_with_embedded_event(&pool).await?;
        let write = write(product_id, source_event_id, EventId::new());
        apply(&pool, &write).await?;

        let outcome = apply(
            &pool,
            &ProductTranslationWrite {
                enrichment_event_id: EventId::new(),
                ..write
            },
        )
        .await?;

        assert_eq!(ProductTranslationWriteOutcome::Duplicate, outcome);
        let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM product_events WHERE product_id = $1 AND event_group = 'ENRICHMENT'",
    )
    .bind(uuid::Uuid::from(product_id))
    .fetch_one(&pool)
    .await?;
        assert_eq!(2, count, "source embedded event plus one translated event");
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation duplicate acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_stale_without_writing_when_product_revision_advanced() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source_event_id) = insert_product_with_embedded_event(&pool).await?;
        let newer_event_id =
            insert_event_and_advance_product(&pool, product_id, "PRODUCT_STATE_CHANGED", "DOMAIN")
                .await?;

        let outcome = apply(&pool, &write(product_id, source_event_id, EventId::new())).await?;

        assert_eq!(ProductTranslationWriteOutcome::Stale, outcome);
        let translation_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM product_translations WHERE product_id = $1",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, translation_count);
        let current_event = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1",
        )
        .bind(uuid::Uuid::from(product_id))
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
    write: &ProductTranslationWrite,
) -> Result<ProductTranslationWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

fn write(
    product_id: ProductId,
    source_event_id: EventId,
    enrichment_event_id: EventId,
) -> ProductTranslationWrite {
    ProductTranslationWrite {
        product_id,
        source_event_id,
        enrichment_event_id,
        source_language: Language::De,
        titles: IndexMap::from([
            (Language::En, Title::from("Antique chair")),
            (Language::Fr, Title::from("Chaise ancienne")),
        ]),
    }
}

async fn insert_product_with_embedded_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductId, EventId), sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Translation shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("translation-shop-{shop_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Antiker Stuhl', 'de', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_id))
        .bind(format!("translation-product-{product_id}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(product_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_id, event_id))
}

async fn insert_event_and_advance_product(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(event_group)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE products SET event_id = $1 WHERE product_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}
