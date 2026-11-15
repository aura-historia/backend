//! The append-only label history of a ListingSource against PostgreSQL: every label set, replaced
//! or cleared appends one entry in the transaction of the change, no entry changes afterwards, and
//! the migration that introduced it keeps every label that stood before
//! (`docs/crawler/README.md#listingsource-scope`).

use crawler::review::model::{ReviewActor, SourceLabel, SourceLabelNote, SourceLabelRequest};
use crawler::review::repository::CrawlerReviewRepository;
use listing_source_core::ListingSourceId;
use sqlx::{Connection, Executor, PgPool};
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

/// The migration that introduced the label history.
const LABEL_HISTORY_MIGRATION: i64 = 20261114100000;

type LabelEventRow = (String, Option<String>, Option<String>, String);

async fn insert_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Shop', 'shop', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    listing_source_id
}

fn label(label: SourceLabel, note: Option<&str>) -> SourceLabelRequest {
    SourceLabelRequest::new(
        label,
        note.map(|note| SourceLabelNote::try_from(note.to_owned()).unwrap()),
    )
    .unwrap()
}

async fn history(pool: &PgPool, listing_source_id: ListingSourceId) -> Vec<LabelEventRow> {
    sqlx::query_as(
        "SELECT event_kind, source_label, source_label_note, actor \
         FROM listing_source_label_events WHERE listing_source_id = $1 \
         ORDER BY occurred_at, event_id",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

fn event(kind: &str, label: Option<&str>, note: Option<&str>, actor: &str) -> LabelEventRow {
    (
        kind.to_owned(),
        label.map(str::to_owned),
        note.map(str::to_owned),
        actor.to_owned(),
    )
}

/// A relabel replaces the source's label and keeps the one it replaced, with its note, in the
/// history; a clear appends one entry, and clearing a source without a label appends none.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_every_replaced_and_cleared_label_in_the_history() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_source(&pool).await;
    let reviews = CrawlerReviewRepository::new(pool.clone());

    reviews
        .label_source(
            &listing_source_id,
            ReviewActor::Human,
            &label(SourceLabel::NotAnEshop, Some("enquiries only")),
        )
        .await
        .unwrap();
    reviews
        .label_source(
            &listing_source_id,
            ReviewActor::AgentAuthor,
            &label(SourceLabel::NotAnAntiqueShop, Some("sells new furniture")),
        )
        .await
        .unwrap();
    reviews
        .clear_source_label(&listing_source_id, ReviewActor::Human)
        .await
        .unwrap();
    reviews
        .clear_source_label(&listing_source_id, ReviewActor::Human)
        .await
        .unwrap();

    assert_eq!(
        history(&pool, listing_source_id).await,
        vec![
            event(
                "LABELLED",
                Some("NOT_AN_ESHOP"),
                Some("enquiries only"),
                "HUMAN"
            ),
            event(
                "LABELLED",
                Some("NOT_AN_ANTIQUE_SHOP"),
                Some("sells new furniture"),
                "AGENT_AUTHOR"
            ),
            event("CLEARED", None, None, "HUMAN"),
        ]
    );
}

/// The database accepts every label the crawler persists, the retired one included, on a source
/// and in its history, and refuses a name no label has.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_exactly_the_labels_the_crawler_persists() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_source(&pool).await;
    let store = |name: &'static str| {
        let pool = pool.clone();
        async move {
            let on_source = sqlx::query(
                "UPDATE listing_sources \
                 SET source_label = $2, source_label_note = 'seen', source_labelled_at = NOW(), \
                     source_labelled_by = 'HUMAN', crawl_enabled = FALSE \
                 WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
            .bind(name)
            .execute(&pool)
            .await;
            let in_history = sqlx::query(
                "INSERT INTO listing_source_label_events \
                     (listing_source_id, event_kind, source_label, source_label_note, actor) \
                 VALUES ($1, 'LABELLED', $2, 'seen', 'HUMAN')",
            )
            .bind(listing_source_id.as_uuid())
            .bind(name)
            .execute(&pool)
            .await;
            (on_source.is_ok(), in_history.is_ok())
        }
    };

    for label in SourceLabel::ALL {
        assert_eq!(store(label.as_str()).await, (true, true), "{label:?}");
    }
    assert_eq!(store("NOT_A_LABEL").await, (false, false));
}

/// No entry of the history can be changed or deleted, only the deletion of its source removes it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_change_or_delete_a_history_entry() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_source(&pool).await;
    CrawlerReviewRepository::new(pool.clone())
        .label_source(
            &listing_source_id,
            ReviewActor::Human,
            &label(SourceLabel::ShopClosed, Some("domain for sale")),
        )
        .await
        .unwrap();

    for statement in [
        "UPDATE listing_source_label_events SET source_label_note = 'changed' \
         WHERE listing_source_id = $1",
        "DELETE FROM listing_source_label_events WHERE listing_source_id = $1",
    ] {
        let error = sqlx::query(statement)
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .expect_err(statement);
        assert!(error.to_string().contains("append-only"), "{error}");
    }
    assert_eq!(history(&pool, listing_source_id).await.len(), 1);

    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(history(&pool, listing_source_id).await.is_empty());
}

/// Applied to a database that already holds labelled sources, the migration changes no source
/// row, keeps the retired APPROACH_NOT_APPLICABLE readable and seeds the history with every label
/// as it was set. It runs in a schema of its own, so the shared test schema stays as it is.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_seed_the_history_with_every_existing_label_and_change_no_source() {
    let pool = get_postgres_client().await;
    let mut connection = pool.acquire().await.unwrap().detach();
    connection
        .execute("DROP SCHEMA IF EXISTS label_history_migration CASCADE")
        .await
        .unwrap();
    connection
        .execute(
            "CREATE SCHEMA label_history_migration; \
             SET search_path TO label_history_migration, public",
        )
        .await
        .unwrap();
    let migrator = sqlx::migrate!("./migrations");
    let (before, from) = migrator
        .iter()
        .partition::<Vec<_>, _>(|migration| migration.version < LABEL_HISTORY_MIGRATION);
    for migration in before {
        connection
            .execute(migration.sql.clone())
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", migration.description));
    }
    connection
        .execute(
            "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
                  source_label, source_label_note, source_labelled_at, source_labelled_by, \
                  created, updated) \
             VALUES \
                 ('00000000-0000-0000-0000-000000000001', 'Gallery', 'gallery', FALSE, \
                  'NOT_AN_ESHOP', NULL, '2026-11-12 10:00:00+00', 'HUMAN', NOW(), NOW()), \
                 ('00000000-0000-0000-0000-000000000002', 'Shop', 'shop', FALSE, \
                  'APPROACH_NOT_APPLICABLE', 'items share one page', '2026-11-13 10:00:00+00', \
                  'AGENT_AUTHOR', NOW(), NOW()), \
                 ('00000000-0000-0000-0000-000000000003', 'Crawled', 'crawled', TRUE, \
                  NULL, NULL, NULL, NULL, NOW(), NOW())",
        )
        .await
        .unwrap();
    let sources_before = source_rows(&mut connection).await;

    for migration in from {
        connection
            .execute(migration.sql.clone())
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", migration.description));
    }

    let sources_after = source_rows(&mut connection).await;
    assert_eq!(
        sources_after
            .iter()
            .map(|after| keep_columns_of(after, &sources_before[0]))
            .collect::<Vec<_>>(),
        sources_before,
        "a later migration may add columns, but no existing value changes"
    );
    let seeded: Vec<(
        uuid::Uuid,
        String,
        String,
        Option<String>,
        String,
        time::OffsetDateTime,
    )> = sqlx::query_as(
        "SELECT listing_source_id, event_kind, source_label, source_label_note, actor, \
                    occurred_at \
             FROM listing_source_label_events ORDER BY listing_source_id",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        seeded
            .iter()
            .map(|row| (
                row.0.as_u128(),
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_deref(),
                row.4.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (1, "LABELLED", "NOT_AN_ESHOP", None, "HUMAN"),
            (
                2,
                "LABELLED",
                "APPROACH_NOT_APPLICABLE",
                Some("items share one page"),
                "AGENT_AUTHOR"
            ),
        ]
    );
    assert_eq!(
        seeded[0].5,
        time::macros::datetime!(2026-11-12 10:00:00 UTC),
        "seeded as it was set"
    );

    connection
        .execute("DROP SCHEMA label_history_migration CASCADE")
        .await
        .unwrap();
    connection.close().await.unwrap();
}

/// Every column of every source, so a comparison sees any change.
async fn source_rows(connection: &mut sqlx::PgConnection) -> Vec<serde_json::Value> {
    sqlx::query_scalar("SELECT to_jsonb(s) FROM listing_sources s ORDER BY listing_source_id")
        .fetch_all(connection)
        .await
        .unwrap()
}

/// `row` with only the columns `columns_of` has.
fn keep_columns_of(row: &serde_json::Value, columns_of: &serde_json::Value) -> serde_json::Value {
    let columns = columns_of.as_object().unwrap();
    serde_json::Value::Object(
        row.as_object()
            .unwrap()
            .iter()
            .filter(|(column, _)| columns.contains_key(*column))
            .map(|(column, value)| (column.clone(), value.clone()))
            .collect(),
    )
}
