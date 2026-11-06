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

    /// The work locks held at this moment, in no particular order: a point-in-time observation
    /// for the console, not a synchronization primitive. URL locks are not listed.
    pub fn running_work(&self) -> Vec<RunningWork> {
        self.locks
            .iter()
            .filter_map(|entry| {
                let (scope, listing_source_id) = WorkLockScope::parse_key(entry.key())?;
                Some(RunningWork {
                    scope,
                    listing_source_id,
                    since: *entry.value(),
                })
            })
            .collect()
    }
}

/// A work lock a component holds right now, as the console reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningWork {
    pub scope: WorkLockScope,
    pub listing_source_id: ListingSourceId,
    /// When the lock was acquired.
    pub since: Instant,
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

    /// The inverse of [`Self::key`]: `None` for a URL lock's key.
    fn parse_key(key: &str) -> Option<(Self, ListingSourceId)> {
        let (scope, id) = key.split_once(':')?;
        let scope = match scope {
            "spider" => Self::Spider,
            "scraper" => Self::Scraper,
            _ => return None,
        };
        Some((scope, id.parse().ok()?))
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
    fn running_work_lists_held_work_locks_with_their_scope_and_not_url_locks() {
        let manager = LocalLockManager::new();
        let spidered = ListingSourceId::new();
        let scraped = ListingSourceId::new();
        let url = url::Url::parse("https://example.com/product/42").unwrap();

        let spider = WorkLock::try_acquire(&manager, WorkLockScope::Spider, spidered);
        let scraper = WorkLock::try_acquire(&manager, WorkLockScope::Scraper, scraped);
        let url_lock = UrlLock::try_acquire(&manager, &url);

        let mut running: Vec<_> = manager
            .running_work()
            .into_iter()
            .map(|work| (work.scope, work.listing_source_id))
            .collect();
        running.sort_by_key(|(scope, _)| *scope as u8);
        assert_eq!(
            running,
            vec![
                (WorkLockScope::Spider, spidered),
                (WorkLockScope::Scraper, scraped)
            ]
        );

        drop((spider, scraper, url_lock));

        assert!(manager.running_work().is_empty());
    }

    #[test]
    fn work_lock_key_round_trips_through_parse_key() {
        let listing_source_id = ListingSourceId::new();

        for scope in [WorkLockScope::Spider, WorkLockScope::Scraper] {
            assert_eq!(
                WorkLockScope::parse_key(&scope.key(listing_source_id)),
                Some((scope, listing_source_id))
            );
        }
        assert_eq!(
            WorkLockScope::parse_key("url:https://example.com/product/42"),
            None
        );
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
