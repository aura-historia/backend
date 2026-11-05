//! Synchronizes crawler-local ListingSource crawl eligibility from business truth.

use crate::network::policy::canonical_crawler_domain;
use crate::service::crawler_domain_configuration::CrawlerDomainConfigurationRepository;
use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// How long an enabled ListingSource may stay without crawler-local domains before the
/// condition is reported as a warning. Domains are registered by administration events
/// after the ListingSource snapshot is applied, so right after startup the state is
/// routinely temporary and only logged at debug level.
const UNCONFIGURED_WARNING_GRACE: Duration = Duration::from_secs(10 * 60);

/// Process-local memory of enabled ListingSources observed without crawler-local domains.
#[derive(Debug, Default)]
struct UnconfiguredListingSources {
    first_seen: HashMap<ListingSourceId, Instant>,
}

/// Classification of one unconfigured observation.
#[derive(Debug, PartialEq, Eq)]
enum UnconfiguredObservation {
    /// First seen less than the grace period ago; most likely bootstrap ordering.
    WithinGrace,
    /// Still unconfigured after the grace period; a genuine configuration gap.
    Overdue,
}

impl UnconfiguredListingSources {
    /// Records the current unconfigured set, forgets sources that gained domains, and
    /// returns each remaining source's classification against `grace`, keyed by id.
    fn observe(
        &mut self,
        unconfigured: &[ListingSourceId],
        now: Instant,
        grace: Duration,
    ) -> HashMap<ListingSourceId, UnconfiguredObservation> {
        self.first_seen
            .retain(|listing_source_id, _| unconfigured.contains(listing_source_id));
        unconfigured
            .iter()
            .map(|listing_source_id| {
                let first_seen = *self.first_seen.entry(*listing_source_id).or_insert(now);
                let observation = if now.saturating_duration_since(first_seen) >= grace {
                    UnconfiguredObservation::Overdue
                } else {
                    UnconfiguredObservation::WithinGrace
                };
                (*listing_source_id, observation)
            })
            .collect()
    }
}

/// Crawler-safe business identity.
///
/// The registration source supplies canonical ListingSource values. `crawl_enabled` is
/// crawler-local state derived from a complete successful `WEB_CRAWL` scope read.
#[derive(Debug, Clone)]
pub struct RegisteredListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub crawl_enabled: bool,
    /// The host of the public site the registration source presents for the shop, when it
    /// names one that is a domain; `None` when the source carries no URL or its host is none.
    /// [Domain seeding](ListingSourceRegistrationService::seeding_domains_from_public_hosts)
    /// registers it for a source without crawler-local domains.
    pub public_host: Option<Domain>,
}

/// Result of one atomically applied business snapshot.
#[derive(Debug, Default)]
pub struct ListingSourceSnapshotResult {
    pub disabled: u64,
    pub enabled_without_domains: Vec<RegisteredListingSource>,
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
    async fn apply_snapshot(
        &self,
        listing_sources: &[RegisteredListingSource],
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
    /// Registers the public host of a source without crawler-local domains as its domain;
    /// `None` leaves every domain to operators.
    domain_seeding: Option<Arc<dyn CrawlerDomainConfigurationRepository>>,
    unconfigured: Mutex<UnconfiguredListingSources>,
}

impl ListingSourceRegistrationService {
    pub fn new(
        source: Box<dyn ListingSourceRegistrationSource>,
        repository: Box<dyn ListingSourceRegistrationRepository>,
    ) -> Self {
        Self {
            source,
            repository,
            domain_seeding: None,
            unconfigured: Mutex::new(UnconfiguredListingSources::default()),
        }
    }

    /// Makes every sync register, through `domains`, the public host of each enabled
    /// ListingSource without crawler-local domains as its crawler domain, so a new crawler
    /// database gets its scope's domains from the registration source instead of by hand. A
    /// host two sources of the scope share is a marketplace rather than a shop's own site, and
    /// a host another source already owns is refused by the registration; both stay with an
    /// operator, as does a source whose registration fails.
    pub fn seeding_domains_from_public_hosts(
        mut self,
        domains: Arc<dyn CrawlerDomainConfigurationRepository>,
    ) -> Self {
        self.domain_seeding = Some(domains);
        self
    }

    #[tracing::instrument(name = "listing_source_registration_sync", skip(self))]
    pub async fn sync(&self) -> Result<usize, ListingSourceSyncError> {
        let listing_sources = self.source.fetch_registered_listing_sources().await?;
        let enabled_count = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .count();

        let result = self.repository.apply_snapshot(&listing_sources).await?;
        if result.disabled > 0 {
            info!(
                disabled = result.disabled,
                "disabled crawler-local listing sources absent from business crawl scope"
            );
        }
        let unconfigured = self
            .seed_domains(&listing_sources, result.enabled_without_domains)
            .await;
        self.report_unconfigured(&unconfigured);

        Ok(enabled_count)
    }

    /// Registers the public host of each of `enabled_without_domains` that no other source of
    /// `scope` shares, and returns the sources still without domains.
    async fn seed_domains(
        &self,
        scope: &[RegisteredListingSource],
        enabled_without_domains: Vec<RegisteredListingSource>,
    ) -> Vec<RegisteredListingSource> {
        let Some(domains) = &self.domain_seeding else {
            return enabled_without_domains;
        };
        let shared_hosts = shared_public_hosts(scope);
        let mut unconfigured = Vec::new();
        for listing_source in enabled_without_domains {
            let Some(host) = listing_source.public_host.clone() else {
                unconfigured.push(listing_source);
                continue;
            };
            if shared_hosts.contains(&canonical_crawler_domain(host.as_str())) {
                warn!(
                    event = "crawler.listing_source_domain_seed_skipped",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    host = %host,
                    "public host is shared by another ListingSource of the scope; register the shop's own domain by hand"
                );
                unconfigured.push(listing_source);
                continue;
            }
            match domains
                .register(listing_source.listing_source_id, host.clone())
                .await
            {
                Ok(configuration) => info!(
                    event = "crawler.listing_source_domain_seeded",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    domain = %configuration.domain,
                    "registered the ListingSource's public host as its crawler domain"
                ),
                Err(error) => {
                    warn!(
                        event = "crawler.listing_source_domain_seed_failed",
                        listing_source_id = %listing_source.listing_source_id,
                        listing_source_name = %listing_source.listing_source_name,
                        host = %host,
                        error = %error,
                        "could not register the ListingSource's public host as its crawler domain"
                    );
                    unconfigured.push(listing_source);
                }
            }
        }
        unconfigured
    }

    /// Logs enabled ListingSources without crawler-local domains. The condition is only a
    /// warning once it has persisted beyond [`UNCONFIGURED_WARNING_GRACE`]; before that it
    /// is the normal bootstrap window in which domain registration has not caught up yet.
    fn report_unconfigured(&self, enabled_without_domains: &[RegisteredListingSource]) {
        let unconfigured_ids = enabled_without_domains
            .iter()
            .map(|listing_source| listing_source.listing_source_id)
            .collect::<Vec<_>>();
        let observations = self
            .unconfigured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(
                &unconfigured_ids,
                Instant::now(),
                UNCONFIGURED_WARNING_GRACE,
            );
        for listing_source in enabled_without_domains {
            match observations.get(&listing_source.listing_source_id) {
                None => {}
                Some(UnconfiguredObservation::Overdue) => warn!(
                    event = "crawler.listing_source_unconfigured",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    "enabled ListingSource still has no crawler-local domains after the bootstrap grace period"
                ),
                Some(UnconfiguredObservation::WithinGrace) => debug!(
                    event = "crawler.listing_source_unconfigured_pending",
                    listing_source_id = %listing_source.listing_source_id,
                    listing_source_name = %listing_source.listing_source_name,
                    "enabled ListingSource has no crawler-local domains yet; waiting for domain registration"
                ),
            }
        }
    }
}

/// The canonical hosts, without `www.`, that more than one source of `scope` presents.
fn shared_public_hosts(scope: &[RegisteredListingSource]) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut shared = HashSet::new();
    for host in scope
        .iter()
        .filter_map(|listing_source| listing_source.public_host.as_ref())
    {
        let canonical = canonical_crawler_domain(host.as_str());
        if !seen.insert(canonical.clone()) {
            shared.insert(canonical);
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
    async fn apply_snapshot(
        &self,
        listing_sources: &[RegisteredListingSource],
    ) -> Result<ListingSourceSnapshotResult, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let enabled_ids = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .map(|listing_source| uuid::Uuid::from(listing_source.listing_source_id))
            .collect::<Vec<_>>();

        for listing_source in listing_sources {
            sqlx::query(
                "INSERT INTO listing_sources \
                    (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
                 VALUES ($1, $2, $3, $4, NOW(), NOW()) \
                 ON CONFLICT (listing_source_id) DO UPDATE SET \
                    listing_source_name = EXCLUDED.listing_source_name, \
                    listing_source_slug = EXCLUDED.listing_source_slug, \
                    crawl_enabled = EXCLUDED.crawl_enabled, \
                    updated = NOW()",
            )
            .bind(uuid::Uuid::from(listing_source.listing_source_id))
            .bind(listing_source.listing_source_name.as_ref())
            .bind(listing_source.listing_source_slug.to_string())
            .bind(listing_source.crawl_enabled)
            .execute(&mut *transaction)
            .await?;
        }

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

        let unconfigured_ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT s.listing_source_id \
             FROM listing_sources s \
             WHERE s.crawl_enabled = TRUE \
               AND NOT EXISTS ( \
                   SELECT 1 FROM listing_source_domains sd \
                   WHERE sd.listing_source_id = s.listing_source_id \
               )",
        )
        .fetch_all(&mut *transaction)
        .await?;

        transaction.commit().await?;

        let enabled_without_domains = listing_sources
            .iter()
            .filter(|listing_source| {
                listing_source.crawl_enabled
                    && unconfigured_ids
                        .contains(&uuid::Uuid::from(listing_source.listing_source_id))
            })
            .cloned()
            .collect();

        Ok(ListingSourceSnapshotResult {
            disabled,
            enabled_without_domains,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source() -> RegisteredListingSource {
        RegisteredListingSource {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            listing_source_slug: ListingSourceSlugId::raw("source")
                .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            crawl_enabled: true,
            public_host: None,
        }
    }

    fn shop(host: &str) -> RegisteredListingSource {
        RegisteredListingSource {
            public_host: Some(
                Domain::try_from(host)
                    .unwrap_or_else(|error| panic!("invalid test host {host}: {error}")),
            ),
            ..listing_source()
        }
    }

    /// Records registrations and refuses the hosts it is told are owned elsewhere.
    struct FakeDomains {
        registered: Mutex<Vec<(ListingSourceId, Domain)>>,
        owned_elsewhere: Vec<Domain>,
    }

    impl FakeDomains {
        fn new(owned_elsewhere: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                registered: Mutex::default(),
                owned_elsewhere: owned_elsewhere
                    .iter()
                    .map(|host| Domain::try_from(*host).expect("valid test host"))
                    .collect(),
            })
        }

        fn registered(&self) -> Vec<(ListingSourceId, Domain)> {
            self.registered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl CrawlerDomainConfigurationRepository for FakeDomains {
        async fn list_for_source(
            &self,
            _listing_source_id: ListingSourceId,
        ) -> Result<
            Vec<crate::service::crawler_domain_configuration::CrawlerDomainConfiguration>,
            crate::service::crawler_domain_configuration::CrawlerDomainConfigurationError,
        > {
            Ok(Vec::new())
        }

        async fn register(
            &self,
            listing_source_id: ListingSourceId,
            domain: Domain,
        ) -> Result<
            crate::service::crawler_domain_configuration::CrawlerDomainConfiguration,
            crate::service::crawler_domain_configuration::CrawlerDomainConfigurationError,
        > {
            use crate::service::crawler_domain_configuration::{
                CrawlerDomainConfiguration, CrawlerDomainConfigurationError,
            };
            if self.owned_elsewhere.contains(&domain) {
                return Err(
                    CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource {
                        domain,
                        requested_listing_source_id: listing_source_id,
                        current_listing_source_id: ListingSourceId::new(),
                    },
                );
            }
            self.registered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((listing_source_id, domain.clone()));
            Ok(CrawlerDomainConfiguration {
                domain_id: crate::CrawlerDomainId::new(),
                listing_source_id,
                domain,
                created: true,
            })
        }

        async fn remove(
            &self,
            _listing_source_id: ListingSourceId,
            _domain_id: crate::CrawlerDomainId,
        ) -> Result<
            crate::service::crawler_domain_configuration::CrawlerDomainRemoval,
            crate::service::crawler_domain_configuration::CrawlerDomainConfigurationError,
        > {
            unreachable!("seeding never removes a domain")
        }
    }

    /// A service whose business read answers `scope` and whose snapshot leaves every source
    /// of `scope` without domains.
    fn seeding_service(
        scope: Vec<RegisteredListingSource>,
        domains: Arc<FakeDomains>,
    ) -> ListingSourceRegistrationService {
        let mut source = MockListingSourceRegistrationSource::new();
        let fetched = scope.clone();
        source
            .expect_fetch_registered_listing_sources()
            .returning(move || {
                let fetched = fetched.clone();
                Box::pin(async move { Ok(fetched) })
            });
        let mut repository = MockListingSourceRegistrationRepository::new();
        repository.expect_apply_snapshot().returning(move |_| {
            let unconfigured = scope.clone();
            Box::pin(async move {
                Ok(ListingSourceSnapshotResult {
                    disabled: 0,
                    enabled_without_domains: unconfigured,
                })
            })
        });
        ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .seeding_domains_from_public_hosts(domains)
    }

    #[tokio::test]
    async fn should_seed_the_public_host_of_a_source_without_domains_and_skip_shared_hosts() {
        let own_shop = shop("www.shop-a.example");
        let scope = vec![
            own_shop.clone(),
            shop("www.marketplace.example"),
            shop("marketplace.example"),
            listing_source(),
        ];
        let domains = FakeDomains::new(&[]);

        let synced = seeding_service(scope, Arc::clone(&domains)).sync().await;

        assert!(matches!(synced, Ok(4)));
        assert_eq!(
            domains.registered(),
            vec![(
                own_shop.listing_source_id,
                Domain::try_from("www.shop-a.example").expect("valid host")
            )]
        );
    }

    #[tokio::test]
    async fn should_keep_seeding_the_other_sources_when_a_registration_is_refused() {
        let refused = shop("owned.example");
        let seeded = shop("free.example");
        let domains = FakeDomains::new(&["owned.example"]);

        let synced = seeding_service(vec![refused, seeded.clone()], Arc::clone(&domains))
            .sync()
            .await;

        assert!(matches!(synced, Ok(2)));
        assert_eq!(
            domains.registered(),
            vec![(
                seeded.listing_source_id,
                Domain::try_from("free.example").expect("valid host")
            )]
        );
    }

    #[tokio::test]
    async fn should_apply_one_complete_snapshot_after_a_successful_business_read() {
        let source_listing_source = listing_source();
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
            .withf(|sources| sources.len() == 1)
            .returning(|_| Box::pin(async { Ok(ListingSourceSnapshotResult::default()) }));

        let count = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .sync()
            .await;
        assert!(matches!(count, Ok(1)));
    }

    #[test]
    fn should_warn_about_unconfigured_listing_source_only_after_grace_period() {
        let grace = Duration::from_secs(60);
        let start = Instant::now();
        let pending = ListingSourceId::new();
        let configured_later = ListingSourceId::new();
        let mut tracker = UnconfiguredListingSources::default();

        let first = tracker.observe(&[pending, configured_later], start, grace);
        assert_eq!(
            first,
            HashMap::from([
                (pending, UnconfiguredObservation::WithinGrace),
                (configured_later, UnconfiguredObservation::WithinGrace),
            ])
        );

        let within_grace = tracker.observe(&[pending], start + grace / 2, grace);
        assert_eq!(
            within_grace,
            HashMap::from([(pending, UnconfiguredObservation::WithinGrace)])
        );

        let overdue = tracker.observe(&[pending, configured_later], start + grace, grace);
        assert_eq!(
            overdue,
            HashMap::from([
                (pending, UnconfiguredObservation::Overdue),
                (configured_later, UnconfiguredObservation::WithinGrace),
            ]),
            "a source that gained domains and lost them again restarts its grace period"
        );

        assert!(tracker.observe(&[], start + grace * 2, grace).is_empty());
        assert!(tracker.first_seen.is_empty());
    }

    #[tokio::test]
    async fn should_track_unconfigured_listing_sources_across_syncs() {
        let unconfigured = listing_source();
        let unconfigured_id = unconfigured.listing_source_id;
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .times(2)
            .returning(move || {
                let source = unconfigured.clone();
                Box::pin(async move { Ok(vec![source]) })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .times(2)
            .returning(|sources| {
                let enabled_without_domains = sources.to_vec();
                Box::pin(async move {
                    Ok(ListingSourceSnapshotResult {
                        disabled: 0,
                        enabled_without_domains,
                    })
                })
            });

        let service = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository));
        assert!(matches!(service.sync().await, Ok(1)));
        let first_seen = service.unconfigured.lock().unwrap().first_seen[&unconfigured_id];

        assert!(matches!(service.sync().await, Ok(1)));
        let tracked = service.unconfigured.lock().unwrap();
        assert_eq!(tracked.first_seen.len(), 1);
        assert_eq!(
            tracked.first_seen[&unconfigured_id], first_seen,
            "the first observation must survive later syncs so the grace period can elapse"
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
