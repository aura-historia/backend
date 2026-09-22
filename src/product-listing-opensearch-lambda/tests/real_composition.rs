use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use opensearch::GetParts;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_opensearch_lambda::{compose_projection_use_case, handler};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client, refresh_index,
};

#[path = "support/projection_write_relay.rs"]
mod projection_write_relay;

use projection_write_relay::ProjectionWriteRelay;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const PRODUCT_LISTINGS_INDEX: &str = "product-listings";

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn should_apply_replay_withdraw_and_restore_through_the_real_lambda_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let use_case =
            compose_projection_use_case(pool.clone(), get_opensearch_client().await.clone());
        let fixture = insert_active_product_with_event(&pool, 7).await?;

        let applied = handler(
            event(fixture.event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(applied.batch_item_failures.is_empty());
        let live = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(7), live["_version"]);
        assert_eq!(
            json!(fixture.event_id.to_string()),
            live["_source"]["eventId"]
        );
        assert_ne!(json!(true), live["_source"]["projectionDeleted"]);

        let duplicate = handler(
            event(fixture.event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(duplicate.batch_item_failures.is_empty());
        assert_eq!(live, stored(fixture.product_listing_id).await?);

        let withdrawn_event_id = withdraw(&pool, fixture.product_listing_id, 8).await?;
        let withdrawn = handler(
            event(withdrawn_event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(withdrawn.batch_item_failures.is_empty());
        let tombstone = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(8), tombstone["_version"]);
        assert_eq!(
            json!({
                "productListingId": fixture.product_listing_id,
                "projectionDeleted": true
            }),
            tombstone["_source"]
        );

        let stale_live = handler(
            event(fixture.event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(stale_live.batch_item_failures.is_empty());
        assert_eq!(tombstone, stored(fixture.product_listing_id).await?);

        let restored_event_id = restore(&pool, fixture.product_listing_id, 9).await?;
        let restored = handler(
            event(restored_event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(restored.batch_item_failures.is_empty());
        let restored = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(9), restored["_version"]);
        assert_eq!(
            json!(restored_event_id.to_string()),
            restored["_source"]["eventId"]
        );
        assert_ne!(json!(true), restored["_source"]["projectionDeleted"]);

        let delayed_withdrawal = handler(
            event(withdrawn_event_id, fixture.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert!(delayed_withdrawal.batch_item_failures.is_empty());
        assert_eq!(restored, stored(fixture.product_listing_id).await?);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real Lambda composition test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn should_retain_missing_source_and_exhausted_records_through_the_real_lambda_path() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let use_case =
            compose_projection_use_case(pool.clone(), get_opensearch_client().await.clone());
        let missing_event = EventId::new();
        let missing_listing = ProductListingId::new();

        let missing = handler(event(missing_event, missing_listing), use_case.as_ref()).await?;
        assert_eq!(vec!["lambda-message"], failure_ids(missing));
        assert!(stored_optional(missing_listing).await?.is_none());

        let missing_fx = insert_sold_product_with_missing_fx_snapshot_event(&pool, 11).await?;
        let missing_fx_response = handler(
            event(missing_fx.event_id, missing_fx.product_listing_id),
            use_case.as_ref(),
        )
        .await?;
        assert_eq!(vec!["lambda-message"], failure_ids(missing_fx_response));
        assert!(
            stored_optional(missing_fx.product_listing_id)
                .await?
                .is_none()
        );

        let exhausted_fixture = insert_active_product_with_event(&pool, 12).await?;
        let exhausted = handler(
            exhausted_event(
                exhausted_fixture.event_id,
                exhausted_fixture.product_listing_id,
            ),
            use_case.as_ref(),
        )
        .await?;
        assert_eq!(vec!["lambda-message"], failure_ids(exhausted));
        assert!(
            stored_optional(exhausted_fixture.product_listing_id)
                .await?
                .is_none()
        );
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "real Lambda retry test failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn should_fence_delayed_live_and_withdrawal_writes_through_the_real_lambda_composition() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let fixture = insert_active_product_with_event(&pool, 20).await?;
        let mut delayed_live_target = ProjectionWriteRelay::pause(target.clone()).await?;
        let delayed_live_use_case =
            compose_projection_use_case(pool.clone(), delayed_live_target.client.clone());
        let delayed_live = tokio::spawn(async move {
            handler(
                event(fixture.event_id, fixture.product_listing_id),
                delayed_live_use_case.as_ref(),
            )
            .await
        });
        delayed_live_target.wait_until_received().await?;

        let withdrawn_event_id = withdraw(&pool, fixture.product_listing_id, 21).await?;
        let current_use_case = compose_projection_use_case(pool.clone(), target.clone());
        let withdrawn = handler(
            event(withdrawn_event_id, fixture.product_listing_id),
            current_use_case.as_ref(),
        )
        .await?;
        assert!(withdrawn.batch_item_failures.is_empty());
        let tombstone = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(21), tombstone["_version"]);
        assert_eq!(json!(true), tombstone["_source"]["projectionDeleted"]);

        assert_eq!(409, delayed_live_target.resume().await?);
        let delayed_live = delayed_live.await??;
        assert!(delayed_live.batch_item_failures.is_empty());
        assert_eq!(tombstone, stored(fixture.product_listing_id).await?);

        let restore_fixture = insert_active_product_with_event(&pool, 30).await?;
        let initial = handler(
            event(restore_fixture.event_id, restore_fixture.product_listing_id),
            current_use_case.as_ref(),
        )
        .await?;
        assert!(initial.batch_item_failures.is_empty());
        let delayed_withdrawal_event_id =
            withdraw(&pool, restore_fixture.product_listing_id, 31).await?;
        let mut delayed_withdrawal_target = ProjectionWriteRelay::pause(target.clone()).await?;
        let delayed_withdrawal_use_case =
            compose_projection_use_case(pool.clone(), delayed_withdrawal_target.client.clone());
        let delayed_withdrawal = tokio::spawn(async move {
            handler(
                event(
                    delayed_withdrawal_event_id,
                    restore_fixture.product_listing_id,
                ),
                delayed_withdrawal_use_case.as_ref(),
            )
            .await
        });
        delayed_withdrawal_target.wait_until_received().await?;

        let restored_event_id = restore(&pool, restore_fixture.product_listing_id, 32).await?;
        let restored = handler(
            event(restored_event_id, restore_fixture.product_listing_id),
            current_use_case.as_ref(),
        )
        .await?;
        assert!(restored.batch_item_failures.is_empty());
        let restored_document = stored(restore_fixture.product_listing_id).await?;
        assert_eq!(json!(32), restored_document["_version"]);
        assert_eq!(
            json!(restored_event_id.to_string()),
            restored_document["_source"]["eventId"]
        );

        assert_eq!(409, delayed_withdrawal_target.resume().await?);
        let delayed_withdrawal = delayed_withdrawal.await??;
        assert!(delayed_withdrawal.batch_item_failures.is_empty());
        assert_eq!(
            restored_document,
            stored(restore_fixture.product_listing_id).await?
        );
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real Lambda target-write race failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()])]
async fn should_retry_after_the_real_target_accepts_a_write_but_its_response_is_lost() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let target = get_opensearch_client().await.clone();
        let fixture = insert_active_product_with_event(&pool, 40).await?;
        let lost_response_target = ProjectionWriteRelay::drop_response(target.clone()).await?;
        let uncertain_use_case =
            compose_projection_use_case(pool.clone(), lost_response_target.client.clone());

        let first = handler(
            event(fixture.event_id, fixture.product_listing_id),
            uncertain_use_case.as_ref(),
        )
        .await?;
        assert_eq!(vec!["lambda-message"], failure_ids(first));
        assert!((200..300).contains(&lost_response_target.finish().await?));
        let accepted = stored(fixture.product_listing_id).await?;
        assert_eq!(json!(40), accepted["_version"]);
        assert_eq!(
            json!(fixture.event_id.to_string()),
            accepted["_source"]["eventId"]
        );

        let retry_use_case = compose_projection_use_case(pool, target);
        let retry = handler(
            event(fixture.event_id, fixture.product_listing_id),
            retry_use_case.as_ref(),
        )
        .await?;
        assert!(retry.batch_item_failures.is_empty());
        assert_eq!(accepted, stored(fixture.product_listing_id).await?);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "real Lambda response-loss retry failed: {result:?}"
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
        "WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Lambda projection fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Lambda projection fixture source', party_id FROM operator",
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

async fn insert_sold_product_with_missing_fx_snapshot_event(
    pool: &sqlx::PgPool,
    projection_version: i64,
) -> Result<ProductFixture, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let missing_fx_rate_id = uuid::Uuid::now_v7();
    let mut transaction = pool.begin().await?;
    let slug = format!("lambda-sold-source-{}", listing_source_id.as_uuid());
    let operator_party_id = uuid::Uuid::now_v7();
    sqlx::query(
        "WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Lambda sold projection fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Lambda sold projection fixture source', party_id FROM operator",
    )
    .bind(operator_party_id)
    .bind(slug)
    .bind(listing_source_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    // PostgreSQL's normal FK prevents a lost immutable snapshot. This synthetic fixture exercises
    // the real reader's defensive missing-snapshot branch without changing production schema.
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, price_kind, price_amount, price_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Lambda sold projection chair', 'en', 'MONETARY', 12345, 'USD', $6, now(), 'SOLD_OUT', 'ACTIVE', 'https://example.test/lambda-sold-projection-chair', '[]', $7)",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug(product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())
    .bind(missing_fx_rate_id)
    .bind(projection_version)
    .execute(&mut *transaction)
    .await?;
    insert_event(
        &mut transaction,
        product_listing_id,
        event_id,
        json!({"availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}}),
    )
    .await?;
    transaction.commit().await?;
    Ok(ProductFixture {
        product_listing_id,
        event_id,
    })
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

fn event(event_id: EventId, product_listing_id: ProductListingId) -> LambdaEvent<SqsEvent> {
    let body = format!(
        r#"{{"schema_version":2,"scope":"product-listing-opensearch","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{event_id}","ordering_key":"product:{product_listing_id}","payload":{{"event_id":"{event_id}","product_listing_id":"{product_listing_id}"}}}}"#,
    );
    event_with_context(body, Duration::from_secs(60))
}

fn exhausted_event(
    event_id: EventId,
    product_listing_id: ProductListingId,
) -> LambdaEvent<SqsEvent> {
    let body = format!(
        r#"{{"schema_version":2,"scope":"product-listing-opensearch","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{event_id}","ordering_key":"product:{product_listing_id}","payload":{{"event_id":"{event_id}","product_listing_id":"{product_listing_id}"}}}}"#,
    );
    event_with_context(body, Duration::ZERO)
}

fn event_with_context(body: String, remaining: Duration) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some("lambda-message".to_owned());
    message.body = Some(body);
    let mut event = SqsEvent::default();
    event.records = vec![message];
    let mut context = Context::default();
    context.deadline =
        epoch_millis().saturating_add(remaining.as_millis().min(u128::from(u64::MAX)) as u64);
    LambdaEvent::new(event, context)
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
    refresh_index(PRODUCT_LISTINGS_INDEX).await;
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(
            PRODUCT_LISTINGS_INDEX,
            &product_listing_id.to_string(),
        ))
        .send()
        .await?;
    if response.status_code().as_u16() == 404 {
        return Ok(None);
    }
    Ok(Some(response.error_for_status_code()?.json().await?))
}

fn failure_ids(response: aws_lambda_events::sqs::SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}

fn product_slug(product_listing_id: ProductListingId) -> String {
    let suffix = product_listing_id.as_uuid().simple().to_string();
    format!("lambda-projection-{}", &suffix[26..])
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}
