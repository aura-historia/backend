//! Synchronizes the crawler-local ListingSources, their crawl eligibility and their crawl roots
//! from business truth (`docs/crawler/README.md#listingsource-scope`).

use crate::crawl_root::{CrawlRoot, CrawlRootRejection};
use async_trait::async_trait;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tracing::{info, warn};

/// Crawler-safe business identity, as the registration source reports it.
///
/// The registration source supplies canonical ListingSource values. `crawl_enabled` is
/// crawler-local state derived from a complete successful `WEB_CRAWL` scope read.
#[derive(Debug, Clone)]
pub struct RegisteredListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub crawl_enabled: bool,
    /// The public site the registration source presents for the shop, untrusted and as given;
    /// `None` when it presents none. The sync derives the source's crawl root from it.
    pub presentation_url: Option<String>,
}

/// A ListingSource of an applied snapshot with the crawl root the sync derived for it; `None`
/// leaves the source uncrawled.
#[derive(Debug, Clone)]
pub struct ScopedListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub crawl_enabled: bool,
    pub crawl_root: Option<CrawlRoot>,
}

/// Why the sync gives a ListingSource of the scope no crawl root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrawlRootSkip {
    /// The registration source presents no URL for the source.
    NoPresentationUrl,
    /// The presentation URL names no crawlable host.
    Uncrawlable(CrawlRootRejection),
    /// Another source of the scope presents the same host, typically a marketplace: crawling it
    /// would walk the whole marketplace for each of them.
    SharedHost(String),
}

/// Result of one atomically applied business snapshot.
#[derive(Debug, Default)]
pub struct ListingSourceSnapshotResult {
    /// Crawl-enabled sources the snapshot no longer holds, now disabled.
    pub disabled: u64,
    /// Sources whose stored crawl root the snapshot replaced or removed.
    pub moved_crawl_roots: u64,
}

/// Reads the authoritative crawler scope.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationSource: Send + Sync {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError>;
}

/// Applies a complete authoritative ListingSource snapshot to crawler-local state.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationRepository: Send + Sync {
    /// Upserts every source of `listing_sources`, keeping a source a human labelled disabled,
    /// disables every crawl-enabled source it does not hold, and moves each source whose stored
    /// crawl root differs to the snapshot's: the move clears the crawl state and frontier of the
    /// old root and keeps the source's URLs, URL pattern and scrape state. All of it commits
    /// together or not at all.
    async fn apply_snapshot(
        &self,
        listing_sources: &[ScopedListingSource],
    ) -> Result<ListingSourceSnapshotResult, sqlx::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceSyncError {
    #[error("failed to read registered listing sources: {0}")]
    FetchError(String),
    #[error("crawler-local database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

pub struct ListingSourceRegistrationService {
    source: Box<dyn ListingSourceRegistrationSource>,
    repository: Box<dyn ListingSourceRegistrationRepository>,
    /// The skip last logged for each source still skipped, so a sync logs a skip only when it
    /// first appears or changes its reason.
    reported_skips: Mutex<HashMap<ListingSourceId, CrawlRootSkip>>,
}

impl ListingSourceRegistrationService {
    pub fn new(
        source: Box<dyn ListingSourceRegistrationSource>,
        repository: Box<dyn ListingSourceRegistrationRepository>,
    ) -> Self {
        Self {
            source,
            repository,
            reported_skips: Mutex::new(HashMap::new()),
        }
    }

    #[tracing::instrument(name = "listing_source_registration_sync", skip(self))]
    pub async fn sync(&self) -> Result<usize, ListingSourceSyncError> {
        let listing_sources = self.source.fetch_registered_listing_sources().await?;
        let enabled_count = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .count();
        let (scope, skips) = scope_crawl_roots(listing_sources);

        let result = self.repository.apply_snapshot(&scope).await?;
        if result.disabled > 0 {
            info!(
                disabled = result.disabled,
                "disabled crawler-local listing sources absent from business crawl scope"
            );
        }
        if result.moved_crawl_roots > 0 {
            info!(
                event = "crawler.listing_source_crawl_roots_moved",
                moved = result.moved_crawl_roots,
                "moved the crawl roots of listing sources whose presentation URL changed"
            );
        }
        self.report_skips(&scope, skips);

        Ok(enabled_count)
    }

    /// Warns once about each crawl-enabled source the sync skipped, again when its reason
    /// changes, and forgets the sources no longer skipped.
    fn report_skips(
        &self,
        scope: &[ScopedListingSource],
        skips: HashMap<ListingSourceId, CrawlRootSkip>,
    ) {
        let mut reported = self
            .reported_skips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reported.retain(|listing_source_id, _| skips.contains_key(listing_source_id));
        for listing_source in scope {
            let Some(skip) = skips.get(&listing_source.listing_source_id) else {
                continue;
            };
            if !listing_source.crawl_enabled
                || reported.get(&listing_source.listing_source_id) == Some(skip)
            {
                continue;
            }
            match skip {
                CrawlRootSkip::NoPresentationUrl => warn!(
                    event = "crawler.listing_source_crawl_skipped",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    reason = "NO_PRESENTATION_URL",
                    "the admin API presents no URL for the ListingSource; it is not crawled"
                ),
                CrawlRootSkip::Uncrawlable(rejection) => warn!(
                    event = "crawler.listing_source_crawl_skipped",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    reason = "UNCRAWLABLE_PRESENTATION_URL",
                    rejection = %rejection,
                    "the ListingSource's presentation URL names no crawlable host; it is not crawled"
                ),
                CrawlRootSkip::SharedHost(host) => warn!(
                    event = "crawler.listing_source_crawl_skipped",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    reason = "SHARED_HOST",
                    host = %host,
                    "another ListingSource of the scope presents the same host; neither is crawled"
                ),
            }
            reported.insert(listing_source.listing_source_id, skip.clone());
        }
    }
}

/// The scope with the crawl root each source's presentation URL derives, and why each source
/// without one has none. A host that more than one crawl-enabled source presents is crawled for
/// none of them.
fn scope_crawl_roots(
    listing_sources: Vec<RegisteredListingSource>,
) -> (
    Vec<ScopedListingSource>,
    HashMap<ListingSourceId, CrawlRootSkip>,
) {
    let derived: Vec<(RegisteredListingSource, Result<CrawlRoot, CrawlRootSkip>)> = listing_sources
        .into_iter()
        .map(|listing_source| {
            let root = match listing_source.presentation_url.as_deref() {
                None => Err(CrawlRootSkip::NoPresentationUrl),
                Some(url) => {
                    CrawlRoot::from_presentation_url(url).map_err(CrawlRootSkip::Uncrawlable)
                }
            };
            (listing_source, root)
        })
        .collect();
    let shared_hosts = shared_hosts(
        derived
            .iter()
            .filter(|(listing_source, _)| listing_source.crawl_enabled)
            .filter_map(|(_, root)| root.as_ref().ok()),
    );

    let mut skips = HashMap::new();
    let scope = derived
        .into_iter()
        .map(|(listing_source, root)| {
            let root = root.and_then(|root| match shared_hosts.contains(root.host()) {
                true => Err(CrawlRootSkip::SharedHost(root.host().to_owned())),
                false => Ok(root),
            });
            let crawl_root = root
                .map_err(|skip| skips.insert(listing_source.listing_source_id, skip))
                .ok();
            ScopedListingSource {
                listing_source_id: listing_source.listing_source_id,
                listing_source_name: listing_source.listing_source_name,
                listing_source_slug: listing_source.listing_source_slug,
                crawl_enabled: listing_source.crawl_enabled,
                crawl_root,
            }
        })
        .collect();
    (scope, skips)
}

/// The crawl hosts more than one of `roots` names.
fn shared_hosts<'a>(roots: impl Iterator<Item = &'a CrawlRoot>) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut shared = HashSet::new();
    for root in roots {
        if !seen.insert(root.host()) {
            shared.insert(root.host().to_owned());
        }
    }
    shared
}

pub struct ListingSourceRegistrationRepositoryImpl {
    pool: PgPool,
}

impl ListingSourceRegistrationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListingSourceRegistrationRepository for ListingSourceRegistrationRepositoryImpl {
    /// Disabling first and releasing moved roots before writing the new ones keeps the
    /// one-host-per-enabled-source index satisfied after every statement, whichever order the
    /// snapshot swaps hosts between sources in.
    async fn apply_snapshot(
        &self,
        listing_sources: &[ScopedListingSource],
    ) -> Result<ListingSourceSnapshotResult, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let enabled_ids = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .map(|listing_source| uuid::Uuid::from(listing_source.listing_source_id))
            .collect::<Vec<_>>();
        let scope_ids = listing_sources
            .iter()
            .map(|listing_source| uuid::Uuid::from(listing_source.listing_source_id))
            .collect::<Vec<_>>();
        let scope_root_urls = listing_sources
            .iter()
            .map(|listing_source| {
                listing_source
                    .crawl_root
                    .as_ref()
                    .map(|root| root.url().to_string())
            })
            .collect::<Vec<_>>();

        let disabled = sqlx::query(
            "UPDATE listing_sources \
             SET crawl_enabled = FALSE, updated = NOW() \
             WHERE crawl_enabled = TRUE \
               AND NOT (listing_source_id = ANY($1::uuid[]))",
        )
        .bind(&enabled_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let moved_ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "UPDATE listing_sources AS s \
             SET crawl_root_url = NULL, \
                 crawl_host = NULL, \
                 last_crawled = NULL, \
                 next_crawl_at = NULL, \
                 crawl_failure_count = 0, \
                 last_crawl_error_kind = NULL, \
                 last_crawl_outcome = NULL, \
                 crawl_truncation_count = 0, \
                 crawl_rate_limit_streak = 0, \
                 crawl_rate_limited_until = NULL, \
                 last_crawl_run_at = NULL, \
                 updated = NOW() \
             FROM UNNEST($1::uuid[], $2::text[]) AS scope(listing_source_id, crawl_root_url) \
             WHERE s.listing_source_id = scope.listing_source_id \
               AND s.crawl_root_url IS DISTINCT FROM scope.crawl_root_url \
               AND s.crawl_root_url IS NOT NULL \
             RETURNING s.listing_source_id",
        )
        .bind(&scope_ids)
        .bind(&scope_root_urls)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM listing_source_crawl_frontier WHERE listing_source_id = ANY($1)")
            .bind(&moved_ids)
            .execute(&mut *transaction)
            .await?;

        for listing_source in listing_sources {
            sqlx::query(
                "INSERT INTO listing_sources \
                    (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
                     crawl_root_url, crawl_host, created, updated) \
                 VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW()) \
                 ON CONFLICT (listing_source_id) DO UPDATE SET \
                    listing_source_name = EXCLUDED.listing_source_name, \
                    listing_source_slug = EXCLUDED.listing_source_slug, \
                    crawl_enabled = EXCLUDED.crawl_enabled \
                        AND listing_sources.source_label IS NULL, \
                    crawl_root_url = EXCLUDED.crawl_root_url, \
                    crawl_host = EXCLUDED.crawl_host, \
                    updated = NOW()",
            )
            .bind(uuid::Uuid::from(listing_source.listing_source_id))
            .bind(listing_source.listing_source_name.as_ref())
            .bind(listing_source.listing_source_slug.to_string())
            .bind(listing_source.crawl_enabled)
            .bind(
                listing_source
                    .crawl_root
                    .as_ref()
                    .map(|root| root.url().as_str()),
            )
            .bind(listing_source.crawl_root.as_ref().map(CrawlRoot::host))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(ListingSourceSnapshotResult {
            disabled,
            moved_crawl_roots: moved_ids.len() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source(presentation_url: Option<&str>) -> RegisteredListingSource {
        RegisteredListingSource {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            listing_source_slug: ListingSourceSlugId::raw("source")
                .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            crawl_enabled: true,
            presentation_url: presentation_url.map(str::to_owned),
        }
    }

    fn root_url(scoped: &ScopedListingSource) -> Option<&str> {
        scoped.crawl_root.as_ref().map(|root| root.url().as_str())
    }

    #[test]
    fn should_derive_crawl_roots_and_skip_hosts_another_source_presents() {
        let own_shop = listing_source(Some("https://www.shop-a.example/en/"));
        let marketplace_www = listing_source(Some("https://www.marketplace.example/shop/1"));
        let marketplace_bare = listing_source(Some("https://marketplace.example/shop/2"));
        let without_url = listing_source(None);
        let uncrawlable = listing_source(Some("https://203.0.113.7/"));
        let ids = [
            own_shop.listing_source_id,
            marketplace_www.listing_source_id,
            marketplace_bare.listing_source_id,
            without_url.listing_source_id,
            uncrawlable.listing_source_id,
        ];

        let (scope, skips) = scope_crawl_roots(vec![
            own_shop,
            marketplace_www,
            marketplace_bare,
            without_url,
            uncrawlable,
        ]);

        assert_eq!(
            scope.iter().map(root_url).collect::<Vec<_>>(),
            vec![
                Some("https://www.shop-a.example/en/"),
                None,
                None,
                None,
                None
            ]
        );
        assert_eq!(
            skips,
            HashMap::from([
                (
                    ids[1],
                    CrawlRootSkip::SharedHost("marketplace.example".to_owned())
                ),
                (
                    ids[2],
                    CrawlRootSkip::SharedHost("marketplace.example".to_owned())
                ),
                (ids[3], CrawlRootSkip::NoPresentationUrl),
                (
                    ids[4],
                    CrawlRootSkip::Uncrawlable(CrawlRootRejection::NoDomainHost)
                ),
            ])
        );
    }

    #[test]
    fn should_not_count_a_disabled_source_as_sharing_a_host() {
        let shop = listing_source(Some("https://shop.example/"));
        let disabled = RegisteredListingSource {
            crawl_enabled: false,
            ..listing_source(Some("https://shop.example/old"))
        };

        let (scope, _) = scope_crawl_roots(vec![shop, disabled]);

        assert_eq!(root_url(&scope[0]), Some("https://shop.example/"));
    }

    #[tokio::test]
    async fn should_apply_one_complete_snapshot_after_a_successful_business_read() {
        let source_listing_source = listing_source(Some("https://shop.example/"));
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(move || {
                let source = source_listing_source.clone();
                Box::pin(async move { Ok(vec![source]) })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .times(1)
            .withf(|sources| {
                sources.len() == 1 && root_url(&sources[0]) == Some("https://shop.example/")
            })
            .returning(|_| Box::pin(async { Ok(ListingSourceSnapshotResult::default()) }));

        let count = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .sync()
            .await;
        assert!(matches!(count, Ok(1)));
    }

    #[test]
    fn should_report_a_skip_once_until_its_reason_changes() {
        let skipped = listing_source(None);
        let skipped_id = skipped.listing_source_id;
        let scope = vec![ScopedListingSource {
            listing_source_id: skipped_id,
            listing_source_name: skipped.listing_source_name.clone(),
            listing_source_slug: skipped.listing_source_slug.clone(),
            crawl_enabled: true,
            crawl_root: None,
        }];
        let service = ListingSourceRegistrationService::new(
            Box::new(MockListingSourceRegistrationSource::new()),
            Box::new(MockListingSourceRegistrationRepository::new()),
        );
        let reported = || {
            service
                .reported_skips
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };

        service.report_skips(
            &scope,
            HashMap::from([(skipped_id, CrawlRootSkip::NoPresentationUrl)]),
        );
        assert_eq!(
            reported(),
            HashMap::from([(skipped_id, CrawlRootSkip::NoPresentationUrl)])
        );

        let shared = CrawlRootSkip::SharedHost("marketplace.example".to_owned());
        service.report_skips(&scope, HashMap::from([(skipped_id, shared.clone())]));
        assert_eq!(reported(), HashMap::from([(skipped_id, shared)]));

        service.report_skips(&scope, HashMap::new());
        assert!(
            reported().is_empty(),
            "a source no longer skipped is forgotten"
        );
    }

    #[tokio::test]
    async fn should_not_apply_snapshot_when_business_read_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError("unavailable".to_owned()))
                })
            });

        let result = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        )
        .sync()
        .await;
        assert!(matches!(result, Err(ListingSourceSyncError::FetchError(_))));
    }
}
