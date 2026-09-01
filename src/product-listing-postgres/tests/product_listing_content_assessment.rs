use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};
use product_listing_postgres::{
    SqlxProductListingContentAssessmentReader, SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::ports::{
    ProductListingContentAssessmentReader, ProductListingContentAssessmentWrite,
    ProductListingContentAssessmentWriteOutcome, ProductListingContentAssessmentWriter,
    ProductListingContentAssessmentWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_assessment_current_after_price_and_enrichment_events() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, content_source_event_id) =
            insert_product_with_created_event(&pool).await?;

        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Applied,
            apply_assessment(&pool, product_listing_id, content_source_event_id).await?
        );

        for (event_type, event_group) in [
            ("PRODUCT_LISTING_PRICE_CHANGED", "DOMAIN"),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
        ] {
            advance_current_event(&pool, product_listing_id, event_type, event_group).await?;
            let assessments = SqlxProductListingContentAssessmentReader::new(pool.clone())
                .find_current_assessments(&[product_listing_id])
                .await?;
            assert_eq!(
                Some(content_source_event_id),
                assessments
                    .get(&product_listing_id)
                    .map(|assessment| assessment.source_event_id),
                "{event_type} must not invalidate the content assessment"
            );
        }

        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "content assessment freshness acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_hide_assessment_when_content_source_event_changes() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, initial_content_source_event_id) =
            insert_product_with_created_event(&pool).await?;

        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Applied,
            apply_assessment(&pool, product_listing_id, initial_content_source_event_id).await?
        );

        advance_content_source_event(&pool, product_listing_id).await?;

        let assessments = SqlxProductListingContentAssessmentReader::new(pool.clone())
            .find_current_assessments(&[product_listing_id])
            .await?;
        assert!(!assessments.contains_key(&product_listing_id));
        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Stale,
            apply_assessment(&pool, product_listing_id, initial_content_source_event_id).await?
        );

        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "content-source assessment invalidation acceptance failed: {result:?}"
    );
}

async fn apply_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    source_event_id: EventId,
) -> Result<ProductListingContentAssessmentWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingContentAssessmentWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&ProductListingContentAssessmentWrite {
            product_listing_id,
            source_event_id,
            decision: Some(ContentPolicyDecision::Allowed),
        })
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let content_source_event_id = EventId::new();
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Content assessment party')")
        .bind(party_id)
        .bind(format!("content-assessment-party-{party_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Content assessment source', $3)")
        .bind(listing_source_id)
        .bind(format!("content-assessment-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $4, $5, 'Assessment chair', 'en', 'Assessment description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(format!(
                    "content-assessment-{}",
                    &product_listing_id.to_string()[..6]
                ))
        .bind(uuid::Uuid::from(content_source_event_id))
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(
        &mut tx,
        product_listing_id,
        content_source_event_id,
        "PRODUCT_LISTING_CREATED",
        "DOMAIN",
    )
    .await?;
    tx.commit().await?;
    Ok((product_listing_id, content_source_event_id))
}

async fn advance_content_source_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(
        &mut tx,
        product_listing_id,
        event_id,
        "PRODUCT_LISTING_TITLE_CHANGED",
        "DOMAIN",
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

async fn advance_current_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    event_type: &str,
    event_group: &str,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(
        &mut tx,
        product_listing_id,
        event_id,
        event_type,
        event_group,
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_listing_id: ProductListingId,
    event_id: EventId,
    event_type: &str,
    event_group: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}
