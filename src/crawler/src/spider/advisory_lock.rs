//! PostgreSQL advisory locks for spider domain crawling and URL-level scraping.
//!
//! Advisory locks are session-scoped: Postgres automatically releases them when the
//! database connection closes. This means a worker crash or network drop releases the
//! lock with zero application-level cleanup — no expiry timeout needed.
//!
//! # Key derivation
//!
//! `pg_try_advisory_lock` takes a single `int8` (64-bit) key.
//!
//! **Domain locks** derive the key from the 128-bit `domain_id` UUID by XOR-folding
//! the upper and lower 64 bits:
//! ```text
//! key = hi_64(uuid) XOR lo_64(uuid)
//! ```
//!
//! **URL locks** derive the key from the URL string using FNV-1a 64-bit hashing,
//! reinterpreted as a signed `i64`. FNV-1a is deterministic, has no external
//! dependencies, and produces a well-distributed 64-bit value over URL strings.
//!
//! # Usage
//!
//! ```rust,ignore
//! // Spider domain lock
//! match DomainAdvisoryLock::try_acquire(&pool, domain_id).await? {
//!     Some(lock) => { do_spider().await; /* lock dropped → unlocked */ }
//!     None => { /* another worker holds the lock */ }
//! }
//!
//! // Scraper URL lock
//! match UrlAdvisoryLock::try_acquire(&pool, &url).await? {
//!     Some(lock) => { do_scrape().await; /* lock dropped → unlocked */ }
//!     None => { /* another worker holds the lock */ }
//! }
//! ```

use sqlx::{PgPool, Postgres, pool::PoolConnection};
use tracing::warn;

// ---------------------------------------------------------------------------
// Key derivation helpers
// ---------------------------------------------------------------------------

/// Derives a stable `i64` advisory-lock key from a UUID.
///
/// XOR-folds the 128-bit UUID into 64 bits so that every distinct UUID maps to a
/// distinct key with high probability while fitting the `pg_try_advisory_lock(int8)`
/// signature.
pub fn domain_id_to_advisory_key(id: uuid::Uuid) -> i64 {
    let bytes = id.as_bytes();
    let hi = i64::from_be_bytes(bytes[..8].try_into().unwrap());
    let lo = i64::from_be_bytes(bytes[8..].try_into().unwrap());
    hi ^ lo
}

/// FNV-1a 64-bit offset basis and prime (no external crate needed).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Derives a stable `i64` advisory-lock key from a URL string using FNV-1a 64-bit.
///
/// FNV-1a is deterministic and produces a well-distributed 64-bit hash value with
/// no external dependencies. The `u64` result is bit-cast to `i64` — Postgres treats
/// the `int8` column as a signed 64-bit integer, but the bit pattern is all that
/// matters for lock identity.
pub fn url_to_advisory_key(url: &url::Url) -> i64 {
    let mut hash = FNV_OFFSET;
    for byte in url.as_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

// ---------------------------------------------------------------------------
// Shared RAII lock implementation
// ---------------------------------------------------------------------------

/// Internal RAII guard: holds one Postgres connection (keeping its session alive so
/// the advisory lock stays valid) and releases the lock on drop.
struct AdvisoryLock {
    conn: Option<PoolConnection<Postgres>>,
    key: i64,
}

impl AdvisoryLock {
    async fn try_acquire(pool: &PgPool, key: i64) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;

        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;

        if acquired {
            Ok(Some(Self {
                conn: Some(conn),
                key,
            }))
        } else {
            Ok(None)
        }
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            let key = self.key;
            // Spawn a best-effort unlock. If this fails the lock will be released
            // anyway when the connection is eventually returned to the pool and
            // the underlying TCP session ends.
            tokio::spawn(async move {
                let result: Result<bool, _> = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
                    .bind(key)
                    .fetch_one(&mut *conn)
                    .await;
                if let Ok(false) | Err(_) = result {
                    warn!(key, "pg_advisory_unlock returned false or failed");
                }
                // `conn` is dropped here → returned to pool
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Public typed wrappers
// ---------------------------------------------------------------------------

/// RAII advisory lock for a spider domain (keyed by `domain_id` UUID).
///
/// Held for the entire duration of a spider crawl run. Released automatically
/// when dropped (session ends → Postgres releases the lock).
pub struct DomainAdvisoryLock(#[allow(dead_code)] AdvisoryLock);

impl DomainAdvisoryLock {
    /// Attempts to acquire a session-level advisory lock for `domain_id`.
    ///
    /// Returns `Some(lock)` if acquired, `None` if another session already holds it
    /// (non-blocking — never waits).
    pub async fn try_acquire(
        pool: &PgPool,
        domain_id: uuid::Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let key = domain_id_to_advisory_key(domain_id);
        Ok(AdvisoryLock::try_acquire(pool, key).await?.map(Self))
    }
}

/// RAII advisory lock for a scraper URL (keyed by FNV-1a 64-bit hash of the URL string).
///
/// Held for the entire duration of a single URL scrape. Released automatically when
/// dropped. Prevents two concurrent workers from scraping the same URL at the same time.
pub struct UrlAdvisoryLock(#[allow(dead_code)] AdvisoryLock);

impl UrlAdvisoryLock {
    /// Attempts to acquire a session-level advisory lock for `url`.
    ///
    /// Returns `Some(lock)` if acquired, `None` if another session already holds it
    /// (non-blocking — never waits).
    pub async fn try_acquire(pool: &PgPool, url: &url::Url) -> Result<Option<Self>, sqlx::Error> {
        let key = url_to_advisory_key(url);
        Ok(AdvisoryLock::try_acquire(pool, key).await?.map(Self))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- domain_id_to_advisory_key ---

    #[test]
    fn domain_key_is_stable_for_same_uuid() {
        let id = uuid::Uuid::new_v4();
        assert_eq!(domain_id_to_advisory_key(id), domain_id_to_advisory_key(id));
    }

    #[test]
    fn domain_key_differs_for_different_uuids() {
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        assert_ne!(domain_id_to_advisory_key(a), domain_id_to_advisory_key(b));
    }

    #[test]
    fn nil_uuid_produces_zero_key() {
        assert_eq!(domain_id_to_advisory_key(uuid::Uuid::nil()), 0i64);
    }

    // --- url_to_advisory_key ---

    #[test]
    fn url_key_is_stable_for_same_url() {
        let url = url::Url::parse("https://example.com/product/42").unwrap();
        assert_eq!(url_to_advisory_key(&url), url_to_advisory_key(&url));
    }

    #[test]
    fn url_key_differs_for_different_urls() {
        let a = url::Url::parse("https://example.com/product/1").unwrap();
        let b = url::Url::parse("https://example.com/product/2").unwrap();
        assert_ne!(url_to_advisory_key(&a), url_to_advisory_key(&b));
    }

    #[test]
    fn url_key_is_nonzero_for_typical_url() {
        // The all-zero FNV result for an empty string would be a degenerate case;
        // a real URL must produce a nonzero key.
        let url = url::Url::parse("https://shop.example.com/item/99").unwrap();
        assert_ne!(url_to_advisory_key(&url), 0i64);
    }
}
