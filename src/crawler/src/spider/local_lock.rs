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

/// RAII lock for a spider or scraper domain, keyed by persisted `domain_id`.
pub struct DomainLock(#[allow(dead_code)] LocalLock);

impl DomainLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        domain_id: CrawlerDomainId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(format!("domain:{domain_id}"))
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
    fn same_domain_is_locked_until_first_guard_is_dropped() {
        let manager = LocalLockManager::new();
        let domain_id = CrawlerDomainId::new();

        let first = DomainLock::try_acquire(&manager, domain_id);
        assert!(first.is_some());
        assert!(DomainLock::try_acquire(&manager, domain_id).is_none());

        drop(first);

        assert!(DomainLock::try_acquire(&manager, domain_id).is_some());
    }

    #[test]
    fn different_domains_can_be_locked_concurrently() {
        let manager = LocalLockManager::new();

        let first = DomainLock::try_acquire(&manager, CrawlerDomainId::new());
        let second = DomainLock::try_acquire(&manager, CrawlerDomainId::new());

        assert!(first.is_some());
        assert!(second.is_some());
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
