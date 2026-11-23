use aura_historia_worker::scopes::product_listing_opensearch::{
    ProductListingOpenSearchJobs, compose_projection_use_case,
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use opensearch::GetParts;
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::{Value, json};
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client, refresh_index,
};
use uuid::Uuid;

#[path = "product_listing_opensearch/projection_write_relay.rs"]
mod projection_write_relay;

use projection_write_relay::ProjectionWriteRelay;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
fn product_listings_index() -> String {
    test_api::TestContext::current()
        .expect("active projection worker test")
        .product_listings_index()
        .expect("scoped product listings index")
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_apply_replay_withdraw_and_restore_through_the_real_worker_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let handler = jobs(
            pool.clone(),
            get_opensearch_client().await.clone(),
            product_listings_index(),
        );
        let fixture = insert_active_product_with_event(&pool, 7).await?;

        let applied = handle(&handler, fixture.event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, applied);
        let live = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(7), live["_version"]);
        assert_eq!(
            json!(fixture.event_id.to_string()),
            live["_source"]["eventId"]
        );
        assert_ne!(json!(true), live["_source"]["projectionDeleted"]);

        let duplicate = handle(&handler, fixture.event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, duplicate);
        assert_eq!(live, stored(fixture.product_listing_id).await?);

        let withdrawn_event_id = withdraw(&pool, fixture.product_listing_id, 8).await?;
        let withdrawn = handle(&handler, withdrawn_event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, withdrawn);
        let tombstone = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(8), tombstone["_version"]);
        assert_eq!(
            json!({
                "productListingId": fixture.product_listing_id,
                "projectionDeleted": true
            }),
            tombstone["_source"]
        );

        let stale_live = handle(&handler, fixture.event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, stale_live);
        assert_eq!(tombstone, stored(fixture.product_listing_id).await?);

        let restored_event_id = restore(&pool, fixture.product_listing_id, 9).await?;
        let restored = handle(&handler, restored_event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, restored);
        let restored = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(9), restored["_version"]);
        assert_eq!(
            json!(restored_event_id.to_string()),
            restored["_source"]["eventId"]
        );
        assert_ne!(json!(true), restored["_source"]["projectionDeleted"]);

        let delayed_withdrawal =
            handle(&handler, withdrawn_event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, delayed_withdrawal);
        assert_eq!(restored, stored(fixture.product_listing_id).await?);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real worker composition test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_retry_missing_source_jobs_through_the_real_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let handler = jobs(
            pool.clone(),
            get_opensearch_client().await.clone(),
            product_listings_index(),
        );
        let missing_event = EventId::new();
        let missing_listing = ProductListingId::new();

        let missing = handle(&handler, missing_event, missing_listing).await;
        assert_eq!(Disposition::Retry(error_class("missing_source")), missing);
        assert!(stored_optional(missing_listing).await?.is_none());
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "real worker retry test failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_keep_unbuyable_listings_out_of_search_through_the_real_worker_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let handler = jobs(
            pool.clone(),
            get_opensearch_client().await.clone(),
            product_listings_index(),
        );
        let fixture = insert_active_product_with_event(&pool, 11).await?;
        let id = fixture.product_listing_id;
        assert_eq!(
            Disposition::Complete,
            handle(&handler, fixture.event_id, id).await
        );
        assert_indexed(id, 11).await?;

        // A document an earlier projection rule indexed at this same version: reprojection
        // replaces it with the tombstone, which is how the backfill removes it.
        sqlx::query(
            "UPDATE product_listings SET availability = 'OUT_OF_STOCK' WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(id))
        .execute(&pool)
        .await?;
        assert_eq!(
            Disposition::Complete,
            handle(&handler, fixture.event_id, id).await
        );
        assert_tombstone(id, 11).await?;

        let back_in_stock = change_availability(&pool, id, "OUT_OF_STOCK", "IN_STOCK", 12).await?;
        assert_eq!(
            Disposition::Complete,
            handle(&handler, back_in_stock, id).await
        );
        assert_indexed(id, 12).await?;

        let sold_out = change_availability(&pool, id, "IN_STOCK", "SOLD_OUT", 13).await?;
        assert_eq!(Disposition::Complete, handle(&handler, sold_out, id).await);
        assert_tombstone(id, 13).await?;

        let stale_in_stock = handle(&handler, back_in_stock, id).await;
        assert_eq!(Disposition::Complete, stale_in_stock);
        assert_tombstone(id, 13).await?;
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real worker discoverability test failed: {result:?}"
    );
}

async fn assert_indexed(
    id: ProductListingId,
    version: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let document = stored(id).await?;
    assert_eq!(json!(version), document["_version"]);
    assert_ne!(json!(true), document["_source"]["projectionDeleted"]);
    Ok(())
}

async fn assert_tombstone(
    id: ProductListingId,
    version: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let document = stored(id).await?;
    assert_eq!(json!(version), document["_version"]);
    assert_eq!(
        json!({"productListingId": id, "projectionDeleted": true}),
        document["_source"]
    );
    Ok(())
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_fence_delayed_live_and_withdrawal_writes_through_the_real_worker_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let fixture = insert_active_product_with_event(&pool, 20).await?;
        let mut delayed_live_target = ProjectionWriteRelay::pause(target.clone()).await?;
        let delayed_live_handler = jobs(
            pool.clone(),
            delayed_live_target.client.clone(),
            product_listings_index(),
        );
        let delayed_live = test_api::spawn_owned(async move {
            handle(
                &delayed_live_handler,
                fixture.event_id,
                fixture.product_listing_id,
            )
            .await
        });
        delayed_live_target.wait_until_received().await?;

        let withdrawn_event_id = withdraw(&pool, fixture.product_listing_id, 21).await?;
        let current_handler = jobs(pool.clone(), target.clone(), product_listings_index());
        let withdrawn = handle(
            &current_handler,
            withdrawn_event_id,
            fixture.product_listing_id,
        )
        .await;
        assert_eq!(Disposition::Complete, withdrawn);
        let tombstone = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(21), tombstone["_version"]);
        assert_eq!(json!(true), tombstone["_source"]["projectionDeleted"]);

        assert_eq!(409, delayed_live_target.resume().await?);
        let delayed_live = delayed_live.await?;
        assert_eq!(Disposition::Complete, delayed_live);
        assert_eq!(tombstone, stored(fixture.product_listing_id).await?);

        let restore_fixture = insert_active_product_with_event(&pool, 30).await?;
        let initial = handle(
            &current_handler,
            restore_fixture.event_id,
            restore_fixture.product_listing_id,
        )
        .await;
        assert_eq!(Disposition::Complete, initial);
        let delayed_withdrawal_event_id =
            withdraw(&pool, restore_fixture.product_listing_id, 31).await?;
        let mut delayed_withdrawal_target = ProjectionWriteRelay::pause(target.clone()).await?;
        let delayed_withdrawal_handler = jobs(
            pool.clone(),
            delayed_withdrawal_target.client.clone(),
            product_listings_index(),
        );
        let delayed_withdrawal = test_api::spawn_owned(async move {
            handle(
                &delayed_withdrawal_handler,
                delayed_withdrawal_event_id,
                restore_fixture.product_listing_id,
            )
            .await
        });
        delayed_withdrawal_target.wait_until_received().await?;

        let restored_event_id = restore(&pool, restore_fixture.product_listing_id, 32).await?;
        let restored = handle(
            &current_handler,
            restored_event_id,
            restore_fixture.product_listing_id,
        )
        .await;
        assert_eq!(Disposition::Complete, restored);
        let restored_document = stored(restore_fixture.product_listing_id).await?;
        assert_eq!(json!(32), restored_document["_version"]);
        assert_eq!(
            json!(restored_event_id.to_string()),
            restored_document["_source"]["eventId"]
        );

        assert_eq!(409, delayed_withdrawal_target.resume().await?);
        let delayed_withdrawal = delayed_withdrawal.await?;
        assert_eq!(Disposition::Complete, delayed_withdrawal);
        assert_eq!(
            restored_document,
            stored(restore_fixture.product_listing_id).await?
        );
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real worker target-write race failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_retry_after_the_real_target_accepts_a_write_but_its_response_is_lost() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let fixture = insert_active_product_with_event(&pool, 40).await?;
        let lost_response_target = ProjectionWriteRelay::drop_response(target.clone()).await?;
        let uncertain_handler = jobs(
            pool.clone(),
            lost_response_target.client.clone(),
            product_listings_index(),
        );

        let first = handle(
            &uncertain_handler,
            fixture.event_id,
            fixture.product_listing_id,
        )
        .await;
        assert_eq!(
            Disposition::Retry(error_class("projection_unavailable")),
            first
        );
        assert!((200..300).contains(&lost_response_target.finish().await?));
        let accepted = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(40), accepted["_version"]);
        assert_eq!(
            json!(fixture.event_id.to_string()),
            accepted["_source"]["eventId"]
        );

        let retry_handler = jobs(pool, target, product_listings_index());
        let retry = handle(&retry_handler, fixture.event_id, fixture.product_listing_id).await;
        assert_eq!(Disposition::Complete, retry);
        assert_eq!(accepted, stored(fixture.product_listing_id).await?);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real worker response-loss retry failed: {result:?}"
    );
}

struct ProductFixture {
    product_listing_id: ProductListingId,
    event_id: EventId,
}

async fn insert_active_product_with_event(
    pool: &sqlx::PgPool,
    projection_version: i64,
) -> Result<ProductFixture, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let mut transaction = pool.begin().await?;
    let slug = format!("lambda-source-{}", listing_source_id.as_uuid());
    let operator_party_id = uuid::Uuid::now_v7();
    sqlx::query(
        "WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Lambda projection fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) SELECT $3, $2, 'Lambda projection fixture source', party_id, 'DEALER' FROM operator",
    )
    .bind(operator_party_id)
    .bind(slug)
    .bind(listing_source_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, price_kind, price_amount, price_currency, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, availability, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Lambda projection chair', 'en', 'Projected by the real Lambda composition', 'en', 'MONETARY', 12345, 'USD', 10000, 'USD', 15000, 'USD', 'AVAILABLE', 'ACTIVE', 'https://example.test/lambda-projection-chair', '[{\"url\": \"https://example.test/images/lambda.jpg\"}]', $6)",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug(product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())
    .bind(projection_version)
    .execute(&mut *transaction)
    .await?;
    insert_event(
        &mut transaction,
        product_listing_id,
        event_id,
        json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
    )
    .await?;
    transaction.commit().await?;
    Ok(ProductFixture {
        product_listing_id,
        event_id,
    })
}

async fn change_availability(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    previous: &str,
    current: &str,
    projection_version: i64,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut transaction = pool.begin().await?;
    insert_event(
        &mut transaction,
        product_listing_id,
        event_id,
        json!({"availability": {"previous": previous, "current": current}}),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, availability = $2, projection_version = $3, version = version + 1, updated = now() WHERE product_listing_id = $4",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(current)
    .bind(projection_version)
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(event_id)
}

async fn withdraw(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    projection_version: i64,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut transaction = pool.begin().await?;
    insert_event(
        &mut transaction,
        product_listing_id,
        event_id,
        json!({"lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}}),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, lifecycle = 'WITHDRAWN', availability = NULL, projection_version = $2, version = version + 1, updated = now() WHERE product_listing_id = $3",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(projection_version)
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(event_id)
}

async fn restore(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    projection_version: i64,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut transaction = pool.begin().await?;
    insert_event(
        &mut transaction,
        product_listing_id,
        event_id,
        json!({"lifecycle": {"transition": "RESTORED"}}),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, lifecycle = 'ACTIVE', availability = 'AVAILABLE', projection_version = $2, version = version + 1, updated = now() WHERE product_listing_id = $3",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(projection_version)
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(event_id)
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_listing_id: ProductListingId,
    event_id: EventId,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Composes the production handler, as the worker does, over the given index and client.
fn jobs(
    pool: sqlx::PgPool,
    open_search: opensearch::OpenSearch,
    index: String,
) -> ProductListingOpenSearchJobs {
    ProductListingOpenSearchJobs::new(compose_projection_use_case(pool, open_search, index))
}

async fn handle(
    handler: &ProductListingOpenSearchJobs,
    event_id: EventId,
    product_listing_id: ProductListingId,
) -> Disposition {
    let body = format!(
        r#"{{"schema_version":1,"scope":"product-listing-opensearch","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{event_id}","ordering_key":"product:{product_listing_id}","payload":{{"event_id":"{event_id}","product_listing_id":"{product_listing_id}"}}}}"#,
    );
    handler
        .handle(&Job::new(JobId::from_uuid(Uuid::now_v7()), 1, None, body))
        .await
}

async fn stored(
    product_listing_id: ProductListingId,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    stored_optional(product_listing_id)
        .await?
        .ok_or_else(|| "missing expected OpenSearch product projection".into())
}

async fn stored_optional(
    product_listing_id: ProductListingId,
) -> Result<Option<Value>, Box<dyn std::error::Error + Send + Sync>> {
    refresh_index(&product_listings_index()).await;
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(
            &product_listings_index(),
            &product_listing_id.to_string(),
        ))
        .send()
        .await?;
    if response.status_code().as_u16() == 404 {
        return Ok(None);
    }
    Ok(Some(response.error_for_status_code()?.json().await?))
}

fn product_slug(product_listing_id: ProductListingId) -> String {
    let suffix = product_listing_id.as_uuid().simple().to_string();
    format!("lambda-projection-{}", &suffix[26..])
}
