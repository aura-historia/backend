//! Failure custody, redrive, inspection and wake-ups.

mod support;

use delivery_outbox::{
    OutboxWakeup, OutboxWakeups, RedriveReport, RedriveSelection, inspect, redrive,
};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use support::{DATABASE, consent_intent, custody, dispatch, execute, outbox, queued_jobs};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

async fn capture_invalid(pool: &PgPool, snapshot: serde_json::Value) {
    sqlx::query(
        "INSERT INTO delivery_outbox (source_table, snapshot_version, snapshot) \
         VALUES ('notification_deliveries', 1, $1)",
    )
    .bind(snapshot)
    .execute(pool)
    .await
    .unwrap();
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redrive_custody_unchanged_under_its_original_outbox_id() {
    let pool = get_postgres_client().await;
    capture_invalid(&pool, json!({"notification_delivery_id": "repaired-later"})).await;
    capture_invalid(&pool, json!({"unknown": 1})).await;
    dispatch(&pool).await;
    let held = custody(&pool).await;
    assert_eq!(2, held.len());

    let report = redrive(&pool, &RedriveSelection::All).await.unwrap();

    let ids: Vec<i64> = held.iter().map(|(id, ..)| *id).collect();
    assert_eq!(
        RedriveReport {
            redriven: ids.clone(),
            not_in_custody: Vec::new()
        },
        report
    );
    let restored: Vec<(i64, serde_json::Value)> =
        sqlx::query_as("SELECT outbox_id, snapshot FROM delivery_outbox ORDER BY outbox_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        held.iter()
            .map(|(id, _, snapshot, _)| (*id, snapshot.clone()))
            .collect::<Vec<_>>(),
        restored
    );
    assert!(custody(&pool).await.is_empty());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redrive_selected_rows_and_report_those_not_in_custody() {
    let pool = get_postgres_client().await;
    capture_invalid(&pool, json!({"a": 1})).await;
    capture_invalid(&pool, json!({"b": 1})).await;
    dispatch(&pool).await;
    let held = custody(&pool).await;
    let (first, second) = (held[0].0, held[1].0);

    let report = redrive(&pool, &RedriveSelection::Rows(vec![second, 999_999]))
        .await
        .unwrap();

    assert_eq!(
        RedriveReport {
            redriven: vec![second],
            not_in_custody: vec![999_999]
        },
        report
    );
    assert_eq!(
        vec![first],
        custody(&pool)
            .await
            .iter()
            .map(|row| row.0)
            .collect::<Vec<_>>()
    );
    assert_eq!(1, outbox(&pool).await.len());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_report_pending_age_and_custody() {
    let pool = get_postgres_client().await;
    let empty = inspect(&mut pool.acquire().await.unwrap()).await.unwrap();
    assert_eq!(
        (0, None, 0),
        (empty.pending, empty.oldest_pending_age, empty.custody)
    );

    capture_invalid(&pool, json!({"a": 1})).await;
    dispatch(&pool).await;
    execute(
        &pool,
        &consent_intent(&uuid::Uuid::now_v7().to_string(), &"ab".repeat(32)).0,
    )
    .await;
    execute(
        &pool,
        "UPDATE delivery_outbox SET enqueued_at = now() - interval '90 seconds'",
    )
    .await;

    let stats = inspect(&mut pool.acquire().await.unwrap()).await.unwrap();
    assert_eq!((1, 1), (stats.pending, stats.custody));
    assert!(stats.oldest_pending_age.unwrap() >= Duration::from_secs(90));
    assert!(queued_jobs(&pool).await.is_empty());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_wake_the_dispatcher_only_when_a_capture_commits() {
    let pool = get_postgres_client().await;
    let mut wakeups = OutboxWakeups::listen(&pool).await.unwrap();
    let insert = consent_intent(&uuid::Uuid::now_v7().to_string(), &"ab".repeat(32)).0;

    let mut rolled_back = pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(insert.clone()))
        .execute(&mut *rolled_back)
        .await
        .unwrap();
    rolled_back.rollback().await.unwrap();
    assert_eq!(
        OutboxWakeup::Poll,
        wakeups.wait(Duration::from_millis(300)).await.unwrap()
    );

    execute(&pool, &insert).await;
    assert_eq!(
        OutboxWakeup::Captured,
        wakeups.wait(Duration::from_secs(5)).await.unwrap()
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_answer_every_capture_that_arrived_while_busy_with_one_wakeup() {
    let pool = get_postgres_client().await;
    let mut wakeups = OutboxWakeups::listen(&pool).await.unwrap();
    for _ in 0..3 {
        capture_invalid(&pool, json!({"unknown": 1})).await;
    }
    // The dispatcher is busy while the committed captures' notifications arrive.
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        OutboxWakeup::Captured,
        wakeups.wait(Duration::from_secs(5)).await.unwrap()
    );
    assert_eq!(
        OutboxWakeup::Poll,
        wakeups.wait(Duration::from_millis(300)).await.unwrap(),
        "the arrived notifications were answered together"
    );
}
