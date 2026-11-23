use aura_historia_ops::sweep_pending_embeddings;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingEmbeddingQueueFactory, SqlxProductListingEmbeddingQueueReader,
    SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::SweepProductListingEmbeddingsHandler;
use sqlx::PgPool;
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Answers every product with a unit vector, or with no answer at all while `unavailable`.
struct Generator {
    unavailable: bool,
}

#[async_trait::async_trait]
impl EmbeddingGenerator for Generator {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        assert_eq!("Antiker Eichenstuhl", title.as_str());
        if self.unavailable {
            return Err(EmbeddingError::DeadlineExceeded {
                deadline: Duration::from_secs(30),
            });
        }
        EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        unreachable!("the sweep embeds products only")
    }
}

fn sweep(
    pool: &PgPool,
    generator: Generator,
) -> SweepProductListingEmbeddingsHandler<
    SqlxProductListingEmbeddingQueueReader,
    Generator,
    SqlxUnitOfWork,
    SqlxProductListingEmbeddingQueueFactory,
    SqlxProductListingEmbeddingWriterFactory,
> {
    SweepProductListingEmbeddingsHandler::new(
        SqlxProductListingEmbeddingQueueReader::new(pool.clone()),
        generator,
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingEmbeddingQueueFactory::new(),
        SqlxProductListingEmbeddingWriterFactory::new(),
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_commit_pending_embeddings_and_complete_their_work() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let listing = seed_pending_embedding(&pool).await?;

        let report =
            sweep_pending_embeddings(&sweep(&pool, Generator { unavailable: false })).await?;

        assert_eq!(serde_json::json!(1), report["swept"]);
        assert_eq!(serde_json::json!(1), report["applied"]);
        let embedded: bool = sqlx::query_scalar(
            "SELECT embedding IS NOT NULL FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(listing)
        .fetch_one(&pool)
        .await?;
        assert!(embedded);
        let work: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_embedding_work")
            .fetch_one(&pool)
            .await?;
        assert_eq!(0, work);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_fail_and_keep_the_work_when_the_provider_answers_nothing() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        seed_pending_embedding(&pool).await?;

        let swept = sweep_pending_embeddings(&sweep(&pool, Generator { unavailable: true })).await;

        assert!(swept.is_err());
        let attempts: Vec<i16> =
            sqlx::query_scalar("SELECT attempts FROM product_listing_embedding_work")
                .fetch_all(&pool)
                .await?;
        assert_eq!(vec![0], attempts, "pending for the next batch round");
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}

/// A discovered listing whose embedding revision waits as pending work; returns the listing.
async fn seed_pending_embedding(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
    let listing = Uuid::now_v7();
    let event = Uuid::now_v7();
    let party = Uuid::now_v7();
    let source = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Sweep party')",
    )
    .bind(party)
    .bind(format!("sweep-party-{party}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, 'Sweep source', $3, 'DEALER')")
        .bind(source)
        .bind(format!("sweep-source-{source}"))
        .bind(party)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(listing)
        .bind(format!("sweep-{}", &listing.simple().to_string()[26..]))
        .bind(event)
        .bind(source)
        .bind(listing.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event)
        .bind(listing)
        .bind(serde_json::json!({"title": {"previous": null, "current": null}}))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_embedding_work (product_listing_id, source_event_id) VALUES ($1, $2)")
        .bind(listing)
        .bind(event)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(listing)
}
