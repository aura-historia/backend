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

    /// Reports whether a component currently holds its work lock on a ListingSource. This is a
    /// point-in-time observation for diagnostics, not a synchronization primitive.
    pub fn is_work_locked(&self, scope: WorkLockScope, listing_source_id: ListingSourceId) -> bool {
        self.locks.contains_key(&scope.key(listing_source_id))
    }
}

/// The crawler component that owns a work lock.
///
/// Spider and scraper hold independent work locks: each component excludes
/// only itself, so both may work on the same ListingSource concurrently while
/// their own per-host throttles bound request pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkLockScope {
    Spider,
    Scraper,
}

impl WorkLockScope {
    fn key(self, listing_source_id: ListingSourceId) -> String {
        match self {
            Self::Spider => format!("spider:{listing_source_id}"),
            Self::Scraper => format!("scraper:{listing_source_id}"),
        }
    }
}

/// RAII lock for one component's work on one ListingSource, keyed by its id and
/// [`WorkLockScope`].
pub struct WorkLock(#[allow(dead_code)] LocalLock);

impl WorkLock {
    pub fn try_acquire(
        lock_manager: &LocalLockManager,
        scope: WorkLockScope,
        listing_source_id: ListingSourceId,
    ) -> Option<Self> {
        lock_manager
            .try_acquire(scope.key(listing_source_id))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_scope_work_is_locked_until_first_guard_is_dropped() {
        for scope in [WorkLockScope::Spider, WorkLockScope::Scraper] {
            let manager = LocalLockManager::new();
            let listing_source_id = ListingSourceId::new();

            let first = WorkLock::try_acquire(&manager, scope, listing_source_id);
            assert!(first.is_some(), "{scope:?}");
            assert!(
                WorkLock::try_acquire(&manager, scope, listing_source_id).is_none(),
                "{scope:?}"
            );
            assert!(
                manager.is_work_locked(scope, listing_source_id),
                "{scope:?}"
            );

            drop(first);

            assert!(
                !manager.is_work_locked(scope, listing_source_id),
                "{scope:?}"
            );
            assert!(
                WorkLock::try_acquire(&manager, scope, listing_source_id).is_some(),
                "{scope:?}"
            );
        }
    }

    #[test]
    fn spider_and_scraper_can_lock_the_same_listing_source_concurrently() {
        let manager = LocalLockManager::new();
        let listing_source_id = ListingSourceId::new();

        let spider = WorkLock::try_acquire(&manager, WorkLockScope::Spider, listing_source_id);
        let scraper = WorkLock::try_acquire(&manager, WorkLockScope::Scraper, listing_source_id);

        assert!(spider.is_some());
        assert!(scraper.is_some());
        assert!(
            WorkLock::try_acquire(&manager, WorkLockScope::Spider, listing_source_id).is_none()
        );
        assert!(
            WorkLock::try_acquire(&manager, WorkLockScope::Scraper, listing_source_id).is_none()
        );
    }

    #[test]
    fn different_listing_sources_can_be_locked_concurrently() {
        let manager = LocalLockManager::new();

        let locks: Vec<_> = [
            WorkLockScope::Spider,
            WorkLockScope::Spider,
            WorkLockScope::Scraper,
            WorkLockScope::Scraper,
        ]
        .into_iter()
        .map(|scope| WorkLock::try_acquire(&manager, scope, ListingSourceId::new()))
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
}
