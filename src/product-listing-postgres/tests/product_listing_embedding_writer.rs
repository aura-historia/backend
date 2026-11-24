mod support;

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use std::time::Duration;

use platform_postgres::SqlxUnitOfWork;
const EMBEDDING_DIMENSIONS: usize = 768;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingEmbeddingWriterFactory;
use product_listing_service::ports::{
    ImageHash, ProductListingEmbeddingConfirmOutcome, ProductListingEmbeddingConfirmation,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteOutcome,
    ProductListingEmbeddingWriter, ProductListingEmbeddingWriterFactory, StagedProductImage,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_store_embedding_append_enrichment_event_and_advance_current_event_without_aggregate_version()
 {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_created_event(&pool).await?;
        let embedding_write = new_write(product_listing_id, source_event_id, EventId::new());
        let outcome = apply(&pool, &embedding_write).await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied, outcome);
        let (embedding, current_event, version, projection_version): (Option<Vec<f32>>, uuid::Uuid, i64, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(Some(vec![0.25; EMBEDDING_DIMENSIONS]), embedding);
        assert_eq!(
            embedding_write.enrichment_event_id.into_uuid(),
            current_event
        );
        assert_eq!(1, version);
        assert_eq!(2, projection_version);
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE event_id = $1")
                .bind(embedding_write.enrichment_event_id.into_uuid())
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            serde_json::json!({
                "sourceEventId": source_event_id.as_uuid().to_string(),
            }),
            payload
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "embedding write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_keep_first_embedding_when_duplicate_completions_overlap() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source_event_id) = insert_product_with_created_event(&pool).await?;
        let first_write = new_write(product_id, source_event_id, EventId::new());
        let mut duplicate_write = new_write(product_id, source_event_id, EventId::new());
        duplicate_write.embedding = vec![0.75; EMBEDDING_DIMENSIONS];
        let mut first = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(first.connection()).await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied,
            SqlxProductListingEmbeddingWriterFactory::new().in_transaction(&mut first)
                .apply(&first_write).await?);
        let duplicate = apply(&pool, &duplicate_write);
        tokio::pin!(duplicate);
        support::assert_blocked(&pool, blocker_pid, 1, duplicate.as_mut()).await?;
        first.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Duplicate,
            tokio::time::timeout(Duration::from_secs(10), duplicate).await??);
        let stored: (Vec<f32>, uuid::Uuid, i64, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!((first_write.embedding, first_write.enrichment_event_id.into_uuid(), 1, 2), stored);
        let events: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT event_id FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'"
        ).bind(product_id.into_uuid()).fetch_all(&pool).await?;
        assert_eq!(vec![first_write.enrichment_event_id.into_uuid()], events);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "concurrent embedding completion: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_late_embedding_completion_after_new_image_revision_commits() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, old_source) = insert_product_with_created_event(&pool).await?;
        let old_write = new_write(product_id, old_source, EventId::new());
        advance_product_current_event(&pool, product_id).await?;
        let new_source: uuid::Uuid = sqlx::query_scalar(
            "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        let new_source = EventId::try_from(new_source)?;
        let new_write = new_write(product_id, new_source, EventId::new());
        let mut newer = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(newer.connection()).await?;
        SqlxProductListingEmbeddingWriterFactory::new().in_transaction(&mut newer)
            .apply(&new_write).await?;
        let late = apply(&pool, &old_write);
        tokio::pin!(late);
        support::assert_blocked(&pool, blocker_pid, 1, late.as_mut()).await?;
        newer.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Stale,
            tokio::time::timeout(Duration::from_secs(10), late).await??);
        let stored: (Vec<f32>, uuid::Uuid, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, projection_version FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!((new_write.embedding, new_write.enrichment_event_id.into_uuid(), 3), stored);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, count);
        Ok(())
    }.await;
    assert!(result.is_ok(), "reversed embedding completion: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_apply_embedding_after_unrelated_newer_event_without_invalidating_image_source() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source) = insert_product_with_created_event(&pool).await?;
        let mut update = pool.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *update).await?;
        let unrelated = EventId::new();
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(unrelated.into_uuid()).bind(product_id.into_uuid())
            .bind(serde_json::json!({"availability": {"previous": "AVAILABLE", "current": "RESERVED"}})).execute(&mut *update).await?;
        sqlx::query("UPDATE product_listings SET current_event_id = $1, availability = 'RESERVED', version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
            .bind(unrelated.into_uuid()).bind(product_id.into_uuid()).execute(&mut *update).await?;
        let write = new_write(product_id, source, EventId::new());
        let completion = apply(&pool, &write);
        tokio::pin!(completion);
        support::assert_blocked(&pool, blocker_pid, 1, completion.as_mut()).await?;
        update.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied,
            tokio::time::timeout(Duration::from_secs(10), completion).await??);
        Ok(())
    }.await;
    assert!(result.is_ok(), "independent image source: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_record_the_input_behind_the_vector_with_its_image_only_when_embedded() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source) = insert_product_with_created_event(&pool).await?;
        sqlx::query(r#"UPDATE product_listings SET description_text = 'Eiche', description_language = 'de', product_images = '[{"url": "https://example.test/a.jpg"}, {"url": "https://example.test/b.jpg"}]' WHERE product_listing_id = $1"#)
            .bind(product_id.into_uuid()).execute(&pool).await?;
        let hash = ImageHash::from_bytes(&[3; ImageHash::BYTES]).unwrap();
        let mut write = new_write(product_id, source, EventId::new());
        write.image = Some(StagedProductImage { hash: Some(hash) });

        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied, apply(&pool, &write).await?);
        assert_eq!(
            (source.into_uuid(), "Antiker Stuhl".to_owned(), Some("Eiche".to_owned()),
             Some("https://example.test/a.jpg".to_owned()), Some(hash.as_bytes().to_vec())),
            recorded_input(&pool, product_id).await?
        );

        advance_product_current_event(&pool, product_id).await?;
        let newer = current_embedding_source(&pool, product_id).await?;
        let without_image = new_write(product_id, newer, EventId::new());
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied, apply(&pool, &without_image).await?);
        let (recorded_source, _, _, image_url, image_hash) = recorded_input(&pool, product_id).await?;
        assert_eq!(
            (newer.into_uuid(), None, None),
            (recorded_source, image_url, image_hash),
            "an image the embedding could not send is not part of its input"
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "recorded embedding input: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_confirm_only_the_held_revision_against_the_compared_input() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, embedded) = insert_product_with_created_event(&pool).await?;
        let write = new_write(product_id, embedded, EventId::new());
        apply(&pool, &write).await?;
        advance_product_current_event(&pool, product_id).await?;
        let revision = current_embedding_source(&pool, product_id).await?;
        let before = listing_state(&pool, product_id).await?;
        let confirmation = ProductListingEmbeddingConfirmation {
            product_listing_id: product_id,
            source_event_id: revision,
            embedded_source_event_id: embedded,
            image_url: None,
        };

        let compared_elsewhere = ProductListingEmbeddingConfirmation {
            embedded_source_event_id: EventId::new(),
            ..confirmation.clone()
        };
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged,
            confirm(&pool, &compared_elsewhere).await?
        );
        let image_added = ProductListingEmbeddingConfirmation {
            image_url: Some(url::Url::parse("https://example.test/new.jpg")?),
            ..confirmation.clone()
        };
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged,
            confirm(&pool, &image_added).await?,
            "a confirmation never adds an image"
        );

        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::Confirmed,
            confirm(&pool, &confirmation).await?
        );
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::Confirmed,
            confirm(&pool, &confirmation).await?,
            "a repeated confirmation"
        );
        assert_eq!(
            revision.into_uuid(),
            recorded_input(&pool, product_id).await?.0
        );
        assert_eq!(
            before,
            listing_state(&pool, product_id).await?,
            "the vector, the listing's events and its projection stay as they are"
        );
        assert_eq!(
            ProductListingEmbeddingWriteOutcome::Duplicate,
            apply(&pool, &new_write(product_id, revision, EventId::new())).await?,
            "the confirmed revision is embedded"
        );

        sqlx::query("UPDATE product_listing_embedding_inputs SET input_version = input_version + 1 WHERE product_listing_id = $1")
            .bind(product_id.into_uuid()).execute(&pool).await?;
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged,
            confirm(&pool, &ProductListingEmbeddingConfirmation {
                embedded_source_event_id: revision,
                ..confirmation.clone()
            }).await?,
            "an input of another version cannot be confirmed"
        );
        assert_eq!(
            ProductListingEmbeddingWriteOutcome::Applied,
            apply(&pool, &new_write(product_id, revision, EventId::new())).await?,
            "nor does it stand for its revision, so a forced re-embed applies"
        );

        advance_product_current_event(&pool, product_id).await?;
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::Superseded,
            confirm(&pool, &confirmation).await?
        );
        sqlx::query("UPDATE product_listings SET embedding = NULL WHERE product_listing_id = $1")
            .bind(product_id.into_uuid())
            .execute(&pool)
            .await?;
        let latest = current_embedding_source(&pool, product_id).await?;
        assert_eq!(
            ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged,
            confirm(
                &pool,
                &ProductListingEmbeddingConfirmation {
                    source_event_id: latest,
                    embedded_source_event_id: revision,
                    ..confirmation.clone()
                }
            )
            .await?,
            "without a vector there is nothing to confirm"
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "embedding confirmation: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_supersede_a_confirmation_that_waited_for_a_newer_revision_to_commit() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, embedded) = insert_product_with_created_event(&pool).await?;
        apply(&pool, &new_write(product_id, embedded, EventId::new())).await?;
        advance_product_current_event(&pool, product_id).await?;
        let revision = current_embedding_source(&pool, product_id).await?;
        let mut update = pool.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *update).await?;
        let newer = EventId::new();
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(newer.into_uuid()).bind(product_id.into_uuid())
            .bind(serde_json::json!({"images": {"previousCount": 0, "currentCount": 0}}))
            .execute(&mut *update).await?;
        sqlx::query("UPDATE product_listings SET current_event_id = $1, embedding_source_event_id = $1 WHERE product_listing_id = $2")
            .bind(newer.into_uuid()).bind(product_id.into_uuid()).execute(&mut *update).await?;
        let confirmation = ProductListingEmbeddingConfirmation {
            product_listing_id: product_id,
            source_event_id: revision,
            embedded_source_event_id: embedded,
            image_url: None,
        };
        let waiting = confirm(&pool, &confirmation);
        tokio::pin!(waiting);
        support::assert_blocked(&pool, blocker_pid, 1, waiting.as_mut()).await?;
        update.commit().await?;

        assert_eq!(ProductListingEmbeddingConfirmOutcome::Superseded,
            tokio::time::timeout(Duration::from_secs(10), waiting).await??);
        assert_eq!(embedded.into_uuid(), recorded_input(&pool, product_id).await?.0);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "confirmation behind a newer revision: {result:?}"
    );
}

async fn confirm(
    pool: &sqlx::PgPool,
    confirmation: &ProductListingEmbeddingConfirmation,
) -> Result<ProductListingEmbeddingConfirmOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingEmbeddingWriterFactory::new()
        .in_transaction(&mut tx)
        .confirm(confirmation)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

type RecordedInput = (
    uuid::Uuid,
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
);

async fn recorded_input(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<RecordedInput, sqlx::Error> {
    sqlx::query_as(
        "SELECT source_event_id, title_text, description_text, image_url, image_hash FROM product_listing_embedding_inputs WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await
}

async fn current_embedding_source(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, Box<dyn std::error::Error>> {
    let source: uuid::Uuid = sqlx::query_scalar(
        "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await?;
    Ok(EventId::try_from(source)?)
}

/// The vector, the current event, the projection version and the number of events.
async fn listing_state(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(Option<Vec<f32>>, uuid::Uuid, i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT embedding, current_event_id, projection_version, (SELECT count(*) FROM product_listing_events event WHERE event.product_listing_id = listing.product_listing_id) FROM product_listings listing WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await
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
        image: None,
    }
}
async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Embedding party')",
    )
    .bind(party_id)
    .bind(format!("embedding-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, 'Embedding source', $3, 'DEALER')").bind(listing_source_id).bind(format!("embedding-source-{listing_source_id}")).bind(party_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')").bind(product_listing_id.into_uuid()).bind(format!(
                "embedding-product-{}",
                &product_listing_id.as_uuid().simple().to_string()[26..]
            )).bind(event_id.into_uuid()).bind(listing_source_id).bind(product_listing_id.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_listing_id.to_string(),
            "title": {"language": "de", "text": "Antiker Stuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": null
        }))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}
async fn advance_product_current_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, embedding_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
