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

/// Gives `listing_source_id` the crawl root `presentation_url` derives, as a scope refresh that
/// holds only this source does: a changed root moves the source and clears its crawl state and
/// frontier (`docs/crawler/README.md#listingsource-scope`).
pub(crate) async fn refresh_scope_with_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
        ScopedListingSource,
    };
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    ListingSourceRegistrationRepositoryImpl::new(pool.clone())
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: listing_source_core::ListingSourceName::try_from("Test source")
                .unwrap_or_else(|error| panic!("valid test name: {error}")),
            listing_source_slug: listing_source_core::ListingSourceSlugId::raw("test-source")
                .unwrap_or_else(|error| panic!("valid test slug: {error}")),
            crawl_enabled: true,
            crawl_root: Some(crawl_root),
        }])
        .await
        .unwrap_or_else(|error| panic!("apply the scope snapshot: {error}"));
}

/// Stores the crawl root `presentation_url` derives on an existing ListingSource row, without
/// the move a scope refresh applies, for a test that only needs a crawlable source.
pub(crate) async fn store_crawl_root(
    pool: &sqlx::PgPool,
    listing_source_id: listing_source_core::ListingSourceId,
    presentation_url: &str,
) {
    let crawl_root = crate::CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"));
    sqlx::query(
        "UPDATE listing_sources SET crawl_root_url = $2, crawl_host = $3 \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(crawl_root.url().as_str())
    .bind(crawl_root.host())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("store the crawl root: {error}"));
}

/// Captures the level and fields of the tracing events logged while its guard is the thread's
/// default subscriber. Use it from tests that assert on log fields.
pub(crate) mod logged_events {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Debug, Clone)]
    pub(crate) struct LoggedEvent {
        pub(crate) level: Level,
        pub(crate) fields: BTreeMap<String, String>,
    }

    impl LoggedEvent {
        pub(crate) fn field(&self, name: &str) -> Option<&str> {
            self.fields.get(name).map(String::as_str)
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct LoggedEvents(Arc<Mutex<Vec<LoggedEvent>>>);

    impl LoggedEvents {
        /// Starts capturing; events are recorded until the guard drops.
        pub(crate) fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
            let events = Self::default();
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::registry().with(events.clone()),
            );
            tracing::callsite::rebuild_interest_cache();
            (events, guard)
        }

        /// The events recorded so far, which are forgotten.
        pub(crate) fn take(&self) -> Vec<LoggedEvent> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }

        pub(crate) fn all(&self) -> Vec<LoggedEvent> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        /// The events that carry `event` as their `event` field.
        pub(crate) fn named(&self, event: &str) -> Vec<LoggedEvent> {
            self.all()
                .into_iter()
                .filter(|logged| logged.field("event") == Some(event))
                .collect()
        }
    }

    struct Fields(BTreeMap<String, String>);

    impl tracing::field::Visit for Fields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LoggedEvents {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = Fields(BTreeMap::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(LoggedEvent {
                    level: *event.metadata().level(),
                    fields: fields.0,
                });
        }
    }
}
