//! Capture and dispatch per selected source, against real PostgreSQL.

mod support;

use delivery_outbox::{Dispatch, snapshot::OutboxSource};
use serde_json::json;
use support::{
    DATABASE, capture, consent_intent, custody, detach_source_foreign_keys, dispatch, dispatcher,
    execute, expected_jobs, outbox, queued_jobs,
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_capture_exactly_the_routed_columns_of_each_source_insert() {
    let pool = get_postgres_client().await;
    detach_source_foreign_keys(&pool).await;

    for source in OutboxSource::ALL {
        let capture = capture(source);
        execute(&pool, &capture.insert).await;

        assert_eq!(
            vec![(source.table().to_owned(), capture.snapshot)],
            outbox(&pool).await,
            "{}",
            source.table()
        );
        execute(&pool, "DELETE FROM delivery_outbox").await;
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_enqueue_the_routed_jobs_of_each_source_insert_and_empty_the_outbox() {
    let pool = get_postgres_client().await;
    detach_source_foreign_keys(&pool).await;

    for source in OutboxSource::ALL {
        let capture = capture(source);
        execute(&pool, &capture.insert).await;

        let report = dispatch(&pool).await;

        let expected = expected_jobs(source, &capture.snapshot);
        assert!(!expected.is_empty(), "{}", source.table());
        assert_eq!(expected, queued_jobs(&pool).await, "{}", source.table());
        assert_eq!(
            (1, expected.len(), 0),
            (report.claimed, report.enqueued, report.custody)
        );
        assert!(outbox(&pool).await.is_empty());
        execute(&pool, "DELETE FROM job_queue_jobs").await;
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_move_an_invalid_snapshot_of_each_source_to_custody_without_blocking_later_ones() {
    let pool = get_postgres_client().await;
    detach_source_foreign_keys(&pool).await;

    for source in OutboxSource::ALL {
        let mut invalid = capture(source).snapshot;
        invalid["email"] = json!("person@example.com");
        sqlx::query(
            "INSERT INTO delivery_outbox (source_table, snapshot_version, snapshot) VALUES ($1, 1, $2)",
        )
        .bind(source.table())
        .bind(&invalid)
        .execute(&pool)
        .await
        .unwrap();
        let valid = capture(source);
        execute(&pool, &valid.insert).await;

        let report = dispatch(&pool).await;

        let custody = custody(&pool).await;
        assert_eq!(1, custody.len(), "{}", source.table());
        let (_, table, snapshot, class) = &custody[0];
        assert_eq!(
            (source.table(), &invalid, "invalid_snapshot"),
            (table.as_str(), snapshot, class.as_str())
        );
        assert_eq!(
            expected_jobs(source, &valid.snapshot),
            queued_jobs(&pool).await
        );
        assert_eq!((2, 1), (report.claimed, report.custody));
        assert!(outbox(&pool).await.is_empty());
        execute(&pool, "DELETE FROM job_queue_jobs").await;
        execute(&pool, "DELETE FROM delivery_outbox_failures").await;
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_dispatch_every_source_again_with_the_same_effect_after_a_crash_before_commit() {
    let pool = get_postgres_client().await;
    detach_source_foreign_keys(&pool).await;
    let mut expected = Vec::new();
    for source in OutboxSource::ALL {
        let capture = capture(source);
        execute(&pool, &capture.insert).await;
        expected.extend(expected_jobs(source, &capture.snapshot));
    }
    expected.sort();
    let pending = outbox(&pool).await;

    let mut crashed = pool.begin().await.unwrap();
    let Dispatch::Dispatched(report) = dispatcher(&pool)
        .dispatch_within(&mut crashed)
        .await
        .unwrap()
    else {
        panic!("no other dispatcher runs");
    };
    assert_eq!(expected.len(), report.enqueued);
    drop(crashed);

    assert_eq!(pending, outbox(&pool).await);
    assert!(queued_jobs(&pool).await.is_empty());

    dispatch(&pool).await;
    assert_eq!(expected, queued_jobs(&pool).await);
    assert!(outbox(&pool).await.is_empty());
    assert!(custody(&pool).await.is_empty());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_capture_inserts_into_the_selected_sources_only() {
    let pool = get_postgres_client().await;

    let captured: Vec<(String, String, i16)> = sqlx::query_as(
        "SELECT trigger.tgrelid::regclass::text, trigger.tgname, trigger.tgtype \
         FROM pg_trigger trigger JOIN pg_proc function ON function.oid = trigger.tgfoid \
         WHERE function.proname = 'aura_delivery_outbox_capture' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let mut sources: Vec<_> = OutboxSource::ALL
        .iter()
        .map(|s| s.table().to_owned())
        .collect();
    sources.sort();
    assert_eq!(
        sources,
        captured
            .iter()
            .map(|(table, _, _)| table.clone())
            .collect::<Vec<_>>()
    );
    // ROW (1) | INSERT (4), AFTER: no UPDATE (16), DELETE (8) or TRUNCATE (32) trigger.
    for (table, trigger, kind) in &captured {
        assert_eq!(5, *kind, "{table}.{trigger}");
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_capture_nothing_on_updates_deletes_or_unselected_tables() {
    let pool = get_postgres_client().await;
    detach_source_foreign_keys(&pool).await;
    for source in OutboxSource::ALL {
        execute(&pool, &capture(source).insert).await;
    }
    execute(&pool, "DELETE FROM delivery_outbox").await;

    for statement in [
        "UPDATE product_listing_events SET event_time = now()",
        "UPDATE product_listing_raw_revisions SET captured_at = now()",
        "UPDATE search_filter_matches SET feedback = true",
        "UPDATE notification_deliveries SET attempt_count = 1",
        "UPDATE marketing_email_consent_sync_intents SET attempt_count = 1",
        "DELETE FROM geo_resolution_wakeups",
        "DELETE FROM listing_geography_wakeups",
        "DELETE FROM listing_shipping_wakeups",
        "DELETE FROM user_identity_deletion_requests",
        "DELETE FROM notification_deliveries",
        "INSERT INTO users (user_id, email, tier, role) VALUES \
         ('0194f1c2-5b6a-7c3d-8e9f-0a1b2c3d4e5f', 'person@example.com', 'FREE', 'USER')",
    ] {
        execute(&pool, statement).await;
        assert!(outbox(&pool).await.is_empty(), "{statement}");
    }
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_keep_each_consent_recipients_intents_in_capture_order() {
    let pool = get_postgres_client().await;
    let (first, second) = ("ab".repeat(32), "cd".repeat(32));
    let mut intents = Vec::new();
    for recipient in [&first, &second, &first, &first, &second] {
        let intent = uuid::Uuid::now_v7().to_string();
        execute(&pool, &consent_intent(&intent, recipient).0).await;
        intents.push((recipient.clone(), intent));
    }

    dispatch(&pool).await;

    let queued: Vec<(String, String)> =
        sqlx::query_as("SELECT group_key, payload FROM job_queue_jobs ORDER BY sequence")
            .fetch_all(&pool)
            .await
            .unwrap();
    for recipient in [&first, &second] {
        let group = format!("marketing-email:{recipient}");
        let expected: Vec<_> = intents
            .iter()
            .filter(|(key, _)| key == recipient)
            .map(|(_, intent)| intent.clone())
            .collect();
        let delivered: Vec<_> = queued
            .iter()
            .filter(|(key, _)| *key == group)
            .map(|(_, body)| body.clone())
            .collect();
        assert_eq!(expected.len(), delivered.len());
        for (intent, body) in expected.iter().zip(&delivered) {
            let typed = user_core_intent(intent);
            assert!(body.contains(&typed), "{group}: {intent} out of order");
        }
    }
}

/// The intent ID as job bodies carry it.
fn user_core_intent(intent: &str) -> String {
    let uuid: uuid::Uuid = intent.parse().unwrap();
    user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId::try_from(uuid)
        .unwrap()
        .to_string()
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_let_only_one_dispatch_transaction_run_at_a_time() {
    let pool = get_postgres_client().await;
    execute(
        &pool,
        &consent_intent(&uuid::Uuid::now_v7().to_string(), &"ab".repeat(32)).0,
    )
    .await;
    let mut holder = pool.begin().await.unwrap();
    assert!(matches!(
        dispatcher(&pool)
            .dispatch_within(&mut holder)
            .await
            .unwrap(),
        Dispatch::Dispatched(_)
    ));

    assert_eq!(
        Dispatch::Busy,
        dispatcher(&pool).dispatch_batch().await.unwrap()
    );

    holder.commit().await.unwrap();
    assert_eq!(1, queued_jobs(&pool).await.len());
}
