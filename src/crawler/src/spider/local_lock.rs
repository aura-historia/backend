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
}

/// RAII lock for spider work on one crawler domain, keyed by persisted `domain_id`.
///
/// Spider and scraper hold independent domain locks: each component excludes
/// only itself, so both may work on the same domain concurrently while their
/// own per-domain throttles bound request pressure.
pub struct SpiderDomainLock(#[allow(dead_code)] LocalLock);

impl SpiderDomainLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        domain_id: CrawlerDomainId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(format!("spider-domain:{domain_id}"))
            .map(Self)
    }
}

/// RAII lock for scraper work on one crawler domain, keyed by persisted `domain_id`.
///
/// See [`SpiderDomainLock`] for why the two components do not share a lock.
pub struct ScraperDomainLock(#[allow(dead_code)] LocalLock);

impl ScraperDomainLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        domain_id: CrawlerDomainId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(format!("scraper-domain:{domain_id}"))
            .map(Self)
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
    fn same_spider_domain_is_locked_until_first_guard_is_dropped() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let first = SpiderDomainLock::try_acquire(&manager, domain_id);
        assert!(first.is_some());
        assert!(SpiderDomainLock::try_acquire(&manager, domain_id).is_none());

        drop(first);

        assert!(SpiderDomainLock::try_acquire(&manager, domain_id).is_some());
    }

    #[test]
    fn same_scraper_domain_is_locked_until_first_guard_is_dropped() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let first = ScraperDomainLock::try_acquire(&manager, domain_id);
        assert!(first.is_some());
        assert!(ScraperDomainLock::try_acquire(&manager, domain_id).is_none());

        drop(first);

        assert!(ScraperDomainLock::try_acquire(&manager, domain_id).is_some());
    }

    #[test]
    fn spider_and_scraper_can_lock_the_same_domain_concurrently() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let spider = SpiderDomainLock::try_acquire(&manager, domain_id);
        let scraper = ScraperDomainLock::try_acquire(&manager, domain_id);

        assert!(spider.is_some());
        assert!(scraper.is_some());
        assert!(SpiderDomainLock::try_acquire(&manager, domain_id).is_none());
        assert!(ScraperDomainLock::try_acquire(&manager, domain_id).is_none());
    }

    #[test]
    fn different_domains_can_be_locked_concurrently() {
        let manager = LocalLockManager::new();

        let first = SpiderDomainLock::try_acquire(&manager, CrawlerDomainId::new());
        let second = SpiderDomainLock::try_acquire(&manager, CrawlerDomainId::new());
        let third = ScraperDomainLock::try_acquire(&manager, CrawlerDomainId::new());
        let fourth = ScraperDomainLock::try_acquire(&manager, CrawlerDomainId::new());

        assert!(first.is_some());
        assert!(second.is_some());
        assert!(third.is_some());
        assert!(fourth.is_some());
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
