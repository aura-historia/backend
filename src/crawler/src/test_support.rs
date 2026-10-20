//! Shared helpers for lib tests that need real crawler-local state.

use crate::runtime::apply_local_migrations;
use sqlx::postgres::PgPoolOptions;
use sqlx::{AssertSqlSafe, PgPool};
use test_api::get_postgres_client;

/// Creates a dedicated database and migrates it like the binaries do; the lib test
/// container's default database hosts the business schema. Returns the admin pool, which
/// must outlive the crawler pool, and the crawler pool.
pub(crate) async fn migrated_crawler_database() -> (PgPool, PgPool) {
    let admin_pool = get_postgres_client().await;
    let database = format!("crawler_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {database}")))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|error| panic!("create crawler test database: {error}"));
    let pool = PgPoolOptions::new()
        .connect_with(
            admin_pool
                .connect_options()
                .as_ref()
                .clone()
                .database(&database),
        )
        .await
        .unwrap_or_else(|error| panic!("connect to crawler test database: {error}"));

    apply_local_migrations(&pool).await;
    (admin_pool, pool)
}
