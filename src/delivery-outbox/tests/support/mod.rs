//! Captured sources, inserts and assertions shared by the outbox integration tests.

#![allow(dead_code)]

use aura_historia_cdc_routing::route_change;
use aura_historia_jobs::wire;
use delivery_outbox::{
    Dispatch, DispatchReport, OutboxDispatcher,
    snapshot::{self, OutboxSource},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::num::NonZeroU16;
use test_api::Postgres;
use uuid::Uuid;

pub const DATABASE: Postgres = Postgres::new("migrations");

/// The tests write source rows without their parents. Foreign keys are dropped from the source
/// tables of the test's own isolated database; checks, defaults and other triggers stay.
pub async fn detach_foreign_keys(pool: &PgPool, tables: &[&str]) {
    for table in tables {
        let constraints: Vec<String> = sqlx::query_scalar(
            "SELECT conname FROM pg_constraint WHERE contype = 'f' AND conrelid = $1::regclass",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap();
        for constraint in constraints {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} DROP CONSTRAINT \"{constraint}\""
            )))
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

pub async fn detach_source_foreign_keys(pool: &PgPool) {
    let tables: Vec<&str> = OutboxSource::ALL
        .iter()
        .map(|source| source.table())
        .collect();
    detach_foreign_keys(pool, &tables).await;
}

fn id() -> String {
    Uuid::now_v7().to_string()
}

/// One captured insert: the statement that writes it and the snapshot it must capture.
pub struct Capture {
    pub source: OutboxSource,
    pub insert: String,
    pub snapshot: Value,
}

/// A valid insert into `source` and exactly the snapshot its trigger captures.
pub fn capture(source: OutboxSource) -> Capture {
    let (insert, snapshot) = match source {
        OutboxSource::ProductListingEvents => {
            let (event, listing) = (id(), id());
            (
                format!(
                    "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, \
                     event_group, event_type_schema_version, payload, event_time) VALUES \
                     ('{event}', '{listing}', 'ENRICHMENT_GEOGRAPHY_UPDATED', 'ENRICHMENT', 1, '{{}}', now())"
                ),
                json!({
                    "event_id": event, "product_listing_id": listing,
                    "event_type": "ENRICHMENT_GEOGRAPHY_UPDATED", "event_group": "ENRICHMENT",
                    "event_type_schema_version": 1, "payload": {}
                }),
            )
        }
        OutboxSource::ProductListingRawRevisions => {
            let (revision, stream) = (id(), id());
            (
                format!(
                    "INSERT INTO product_listing_raw_revisions (product_listing_raw_revision_id, \
                     product_listing_raw_stream_id, revision, operation, payload_format, \
                     payload_schema_version, raw_values_schema_version, source_payload, raw_values, \
                     normalization_context, provenance, input_sha256, captured_at) VALUES \
                     ('{revision}', '{stream}', 2, 'UPSERT', 'SHOPIFY_PRODUCT', 1, 1, \
                     '{{\"title\": \"raw evidence\"}}', '{{}}', '{{}}', '{{}}', sha256('x'), now())"
                ),
                json!({
                    "product_listing_raw_revision_id": revision,
                    "product_listing_raw_stream_id": stream, "revision": 2
                }),
            )
        }
        OutboxSource::SearchFilterMatches => {
            let (user, filter, listing, event) = (id(), id(), id(), id());
            (
                format!(
                    "INSERT INTO search_filter_matches (user_id, user_search_filter_id, \
                     product_listing_id, origin_event_id, user_search_filter_name) VALUES \
                     ('{user}', '{filter}', '{listing}', '{event}', 'My filter')"
                ),
                json!({
                    "user_id": user, "user_search_filter_id": filter,
                    "product_listing_id": listing, "origin_event_id": event
                }),
            )
        }
        OutboxSource::NotificationDeliveries => {
            let delivery = id();
            (
                format!(
                    "INSERT INTO notification_deliveries (notification_delivery_id, \
                     notification_id, channel, target_key) VALUES \
                     ('{delivery}', '{}', 'EMAIL', 'RECIPIENT')",
                    id()
                ),
                json!({"notification_delivery_id": delivery}),
            )
        }
        OutboxSource::MarketingEmailConsentSyncIntents => {
            let intent = id();
            consent_intent(&intent, &"ab".repeat(32))
        }
        OutboxSource::GeoResolutionWakeups => {
            let resolution = id();
            (
                format!(
                    "INSERT INTO geo_resolution_wakeups (resolution_id, generation) \
                     VALUES ('{resolution}', 3)"
                ),
                json!({"resolution_id": resolution, "generation": 3}),
            )
        }
        OutboxSource::ListingGeographyWakeups | OutboxSource::ListingShippingWakeups => {
            let work = id();
            (
                format!(
                    "INSERT INTO {} (work_id, generation) VALUES ('{work}', 3)",
                    source.table()
                ),
                json!({"work_id": work, "generation": 3}),
            )
        }
        OutboxSource::UserIdentityDeletionRequests => {
            let user = id();
            (
                format!("INSERT INTO user_identity_deletion_requests (user_id) VALUES ('{user}')"),
                json!({"user_id": user}),
            )
        }
    };
    Capture {
        source,
        insert,
        snapshot,
    }
}

/// A consent intent insert. Email address, source and lease state stay out of the snapshot.
pub fn consent_intent(intent: &str, recipient_key: &str) -> (String, Value) {
    (
        format!(
            "INSERT INTO marketing_email_consent_sync_intents (intent_id, source_key, \
             subject_type, source, email, recipient_key, desired, changed_at) VALUES \
             ('{intent}', 'source-{intent}', 'EMAIL_ONLY', 'EMAIL_ONLY_WITHDRAWAL', \
             'person@example.com', '{recipient_key}', false, now())"
        ),
        json!({"intent_id": intent, "recipient_key": recipient_key}),
    )
}

pub async fn execute(pool: &PgPool, statement: &str) {
    sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(pool)
        .await
        .unwrap();
}

/// The jobs routing makes of a snapshot, as (scope, body): what the dispatcher must enqueue.
pub fn expected_jobs(source: OutboxSource, snapshot: &Value) -> Vec<(String, String)> {
    let change =
        snapshot::decode(source.table(), snapshot::SNAPSHOT_VERSION, snapshot.clone()).unwrap();
    let mut jobs: Vec<_> = route_change(&change)
        .unwrap()
        .iter()
        .map(|job| {
            let prepared = wire::prepare(job).unwrap();
            (
                prepared.scope().as_str().to_owned(),
                prepared.body().to_owned(),
            )
        })
        .collect();
    jobs.sort();
    jobs
}

/// Every queued job as (scope, body), sorted.
pub async fn queued_jobs(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT scope, payload FROM job_queue_jobs ORDER BY scope, payload")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Every pending snapshot as (source table, snapshot), in outbox order.
pub async fn outbox(pool: &PgPool) -> Vec<(String, Value)> {
    sqlx::query_as("SELECT source_table, snapshot FROM delivery_outbox ORDER BY outbox_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Every custody row as (outbox ID, source table, snapshot, error class), in outbox order.
pub async fn custody(pool: &PgPool) -> Vec<(i64, String, Value, String)> {
    sqlx::query_as(
        "SELECT outbox_id, source_table, snapshot, error_class \
         FROM delivery_outbox_failures ORDER BY outbox_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

pub fn dispatcher(pool: &PgPool) -> OutboxDispatcher {
    OutboxDispatcher::new(pool.clone(), NonZeroU16::new(50).unwrap())
}

pub async fn dispatch(pool: &PgPool) -> DispatchReport {
    match dispatcher(pool).dispatch_batch().await.unwrap() {
        Dispatch::Dispatched(report) => report,
        Dispatch::Busy => panic!("no other dispatcher runs"),
    }
}
