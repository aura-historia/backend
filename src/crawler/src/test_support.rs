//! Real SQLx crawler migrations on an owned, initially empty database.

use crate::runtime::apply_local_migrations;
use test_api::{IntegrationTestService, Postgres, get_postgres_client};

pub(crate) struct CrawlerDatabase;

#[async_trait::async_trait]
impl IntegrationTestService for CrawlerDatabase {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        // Keep the actual SQLx migrator and ledger; raw schema SQL is not equivalent.
        Postgres::new("src/crawler/tests/fixtures/empty_database")
            .set_up()
            .await;
        let pool = get_postgres_client().await;
        apply_local_migrations(&pool).await;
    }
}
