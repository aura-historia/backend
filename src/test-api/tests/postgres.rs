use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/test-api/tests/fixtures/rds_migrations");
const POSTGRES_WITH_SETUP: Postgres = Postgres::with_setup_script(
    "src/test-api/tests/fixtures/rds_migrations",
    "src/test-api/tests/fixtures/postgres_setup.sql",
);

#[aura_integration_test(services = [POSTGRES])]
async fn should_run_without_errors() {}

#[aura_integration_test(services = [POSTGRES])]
async fn should_create_tables_from_migrations_dir() {
    let pool = get_postgres_client().await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name \
         FROM information_schema.tables \
         WHERE table_schema = 'public' \
           AND table_type = 'BASE TABLE' \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(tables.contains(&"test_items".to_string()));
    assert!(tables.contains(&"test_tags".to_string()));
}

#[aura_integration_test(services = [POSTGRES_WITH_SETUP])]
async fn should_run_setup_script_after_migrations() {
    let pool = get_postgres_client().await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_items WHERE name = $1")
        .bind("from-setup-script")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(1, count);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_insert_and_read_row() {
    let pool = get_postgres_client().await;

    let id: i32 =
        sqlx::query_scalar("INSERT INTO test_items (name, value) VALUES ($1, $2) RETURNING id")
            .bind("widget")
            .bind(42)
            .fetch_one(&pool)
            .await
            .unwrap();

    let (name, value): (String, i32) =
        sqlx::query_as("SELECT name, value FROM test_items WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!("widget", name);
    assert_eq!(42, value);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_clean_up_rows_between_tests() {
    let pool = get_postgres_client().await;

    // This test verifies tear_down ran after the previous test: the table must be empty.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_items")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(0, count);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_handle_foreign_key_relations() {
    let pool = get_postgres_client().await;

    let item_id: i32 =
        sqlx::query_scalar("INSERT INTO test_items (name, value) VALUES ($1, $2) RETURNING id")
            .bind("linked-item")
            .bind(10)
            .fetch_one(&pool)
            .await
            .unwrap();

    sqlx::query("INSERT INTO test_tags (item_id, tag) VALUES ($1, $2), ($1, $3)")
        .bind(item_id)
        .bind("alpha")
        .bind("beta")
        .execute(&pool)
        .await
        .unwrap();

    let tags: Vec<String> =
        sqlx::query_scalar("SELECT tag FROM test_tags WHERE item_id = $1 ORDER BY tag")
            .bind(item_id)
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(vec!["alpha", "beta"], tags);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_cascade_delete_tags_when_item_is_removed() {
    let pool = get_postgres_client().await;

    let item_id: i32 =
        sqlx::query_scalar("INSERT INTO test_items (name, value) VALUES ($1, $2) RETURNING id")
            .bind("to-be-deleted")
            .bind(0)
            .fetch_one(&pool)
            .await
            .unwrap();

    sqlx::query("INSERT INTO test_tags (item_id, tag) VALUES ($1, $2)")
        .bind(item_id)
        .bind("orphan")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM test_items WHERE id = $1")
        .bind(item_id)
        .execute(&pool)
        .await
        .unwrap();

    let tag_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_tags WHERE item_id = $1")
        .bind(item_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(0, tag_count);
}
