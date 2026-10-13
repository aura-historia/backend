use crate::CrawlerDomainId;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use listing_source_core::ListingSourceId;
use std::sync::Arc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Shared process-local RAII lock implementation
// ---------------------------------------------------------------------------

/// Internal RAII guard backed by an in-memory key map.
struct LocalLock {
    locks: Arc<DashMap<String, Instant>>,
    key: String,
}

impl LocalLock {
    fn try_acquire(locks: Arc<DashMap<String, Instant>>, key: String) -> Option<Self> {
        let acquired = match locks.entry(key.clone()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                entry.insert(Instant::now());
                Some(())
            }
        };
        acquired.map(|()| Self { locks, key })
    }
}

impl Drop for LocalLock {
    fn drop(&mut self) {
        self.locks.remove(&self.key);
    }
}

#[derive(Clone, Default)]
pub struct LocalLockManager {
    locks: Arc<DashMap<String, Instant>>,
}

impl LocalLockManager {
    /// Creates the lock manager shared by all crawler workers in one process.
    pub fn new() -> Self {
        Self {
            locks: Arc::new(DashMap::new()),
        }
    }

    fn try_acquire(&self, key: String) -> Option<LocalLock> {
        LocalLock::try_acquire(Arc::clone(&self.locks), key)
    }

    /// Reports whether a component currently holds its domain lock. This is a
    /// point-in-time observation for diagnostics, not a synchronization primitive.
    pub fn is_domain_locked(&self, scope: DomainLockScope, domain_id: CrawlerDomainId) -> bool {
        self.locks.contains_key(&scope.key(domain_id))
    }
}

/// The crawler component that owns a domain lock.
///
/// Spider and scraper hold independent domain locks: each component excludes
/// only itself, so both may work on the same domain concurrently while their
/// own per-domain throttles bound request pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainLockScope {
    Spider,
    Scraper,
}

impl DomainLockScope {
    fn key(self, domain_id: CrawlerDomainId) -> String {
        match self {
            Self::Spider => format!("spider-domain:{domain_id}"),
            Self::Scraper => format!("scraper-domain:{domain_id}"),
        }
    }
}

/// RAII lock for one component's work on one crawler domain, keyed by persisted
/// `domain_id` and [`DomainLockScope`].
pub struct DomainLock(#[allow(dead_code)] LocalLock);

impl DomainLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        scope: DomainLockScope,
        domain_id: CrawlerDomainId,
    ) -> Option<Self> {
        lock_manager.try_acquire(scope.key(domain_id)).map(Self)
    }
}

/// RAII lock for scraper work scoped to one URL.
pub struct UrlLock(#[allow(dead_code)] LocalLock);

impl UrlLock {
    pub fn try_acquire(lock_manager: &LocalLockManager, url: &url::Url) -> Option<Self> {
        lock_manager.try_acquire(format!("url:{url}")).map(Self)
    }
}

/// RAII lock for scraper work scoped to a ListingSource.
pub struct ListingSourceLock(#[allow(dead_code)] LocalLock);

impl ListingSourceLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        listing_source_id: ListingSourceId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(format!("listing-source:{listing_source_id}"))
            .map(Self)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_scope_domain_is_locked_until_first_guard_is_dropped() {
        for scope in [DomainLockScope::Spider, DomainLockScope::Scraper] {
            let manager = LocalLockManager::new();
            let domain_id = CrawlerDomainId::new();

            let first = DomainLock::try_acquire(&manager, scope, domain_id);
            assert!(first.is_some(), "{scope:?}");
            assert!(
                DomainLock::try_acquire(&manager, scope, domain_id).is_none(),
                "{scope:?}"
            );
            assert!(manager.is_domain_locked(scope, domain_id), "{scope:?}");

            drop(first);

            assert!(!manager.is_domain_locked(scope, domain_id), "{scope:?}");
            assert!(
                DomainLock::try_acquire(&manager, scope, domain_id).is_some(),
                "{scope:?}"
            );
        }
    }

    #[test]
    fn spider_and_scraper_can_lock_the_same_domain_concurrently() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let spider = DomainLock::try_acquire(&manager, DomainLockScope::Spider, domain_id);
        let scraper = DomainLock::try_acquire(&manager, DomainLockScope::Scraper, domain_id);

        assert!(spider.is_some());
        assert!(scraper.is_some());
        assert!(DomainLock::try_acquire(&manager, DomainLockScope::Spider, domain_id).is_none());
        assert!(DomainLock::try_acquire(&manager, DomainLockScope::Scraper, domain_id).is_none());
    }

    #[test]
    fn different_domains_can_be_locked_concurrently() {
        let manager = LocalLockManager::new();

        let locks: Vec<_> = [
            DomainLockScope::Spider,
            DomainLockScope::Spider,
            DomainLockScope::Scraper,
            DomainLockScope::Scraper,
        ]
        .into_iter()
        .map(|scope| DomainLock::try_acquire(&manager, scope, CrawlerDomainId::new()))
        .collect();

        assert!(locks.iter().all(Option::is_some));
    }

    #[test]
    fn url_is_locked_until_first_guard_is_dropped() {
        let manager = LocalLockManager::new();
        let url = url::Url::parse("https://example.com/product/42").unwrap();

        let first = UrlLock::try_acquire(&manager, &url);
        assert!(first.is_some());
        assert!(UrlLock::try_acquire(&manager, &url).is_none());

        drop(first);

        assert!(UrlLock::try_acquire(&manager, &url).is_some());
    }

    #[test]
    fn listing_source_is_locked_until_first_guard_is_dropped() {
        let manager = LocalLockManager::new();
        let listing_source_id = ListingSourceId::new();

        let first = ListingSourceLock::try_acquire(&manager, listing_source_id);
        assert!(first.is_some());
        assert!(ListingSourceLock::try_acquire(&manager, listing_source_id).is_none());

        drop(first);

        assert!(ListingSourceLock::try_acquire(&manager, listing_source_id).is_some());
    }
}
