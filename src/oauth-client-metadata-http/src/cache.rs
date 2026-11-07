use oauth_core::metadata_document_client::ClientMetadataDocumentClaims;
use reqwest::header::{CACHE_CONTROL, DATE, EXPIRES, HeaderMap};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// How long a fetched document may be reused: its HTTP cache lifetime, raised to `min` so
/// a client cannot make Aura refetch on every request, and capped at `max` so a changed or
/// withdrawn document takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifetimeBounds {
    pub(crate) min: Duration,
    pub(crate) max: Duration,
}

impl LifetimeBounds {
    /// `Cache-Control: max-age` takes precedence over `Expires`; `no-store`, `no-cache` and an
    /// absent or unparsable lifetime yield the minimum.
    pub(crate) fn lifetime(self, headers: &HeaderMap, now: SystemTime) -> Duration {
        header_lifetime(headers, now)
            .unwrap_or(Duration::ZERO)
            .clamp(self.min, self.max)
    }
}

fn header_lifetime(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    if let Some(cache_control) = headers
        .get(CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
    {
        let mut max_age = None;
        for directive in cache_control.split(',').map(str::trim) {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            match name.trim().to_ascii_lowercase().as_str() {
                "no-store" | "no-cache" => return Some(Duration::ZERO),
                "max-age" => {
                    max_age = value.trim().trim_matches('"').parse::<u64>().ok();
                }
                _ => {}
            }
        }
        if let Some(seconds) = max_age {
            return Some(Duration::from_secs(seconds));
        }
    }
    let expires = http_date(headers.get(EXPIRES)?)?;
    let date = headers.get(DATE).and_then(http_date).unwrap_or(now);
    Some(expires.duration_since(date).unwrap_or(Duration::ZERO))
}

fn http_date(value: &reqwest::header::HeaderValue) -> Option<SystemTime> {
    httpdate::parse_http_date(value.to_str().ok()?).ok()
}

/// Per-process cache of fetched documents. An entry is never served after it expires.
pub(crate) struct DocumentCache {
    entries: Mutex<HashMap<String, CachedDocument>>,
    capacity: usize,
}

struct CachedDocument {
    claims: ClientMetadataDocumentClaims,
    expires: Instant,
}

impl DocumentCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub(crate) fn get(
        &self,
        client_id: &str,
        now: Instant,
    ) -> Option<ClientMetadataDocumentClaims> {
        let mut entries = self.lock();
        match entries.get(client_id) {
            Some(entry) if entry.expires > now => Some(entry.claims.clone()),
            Some(_) => {
                entries.remove(client_id);
                None
            }
            None => None,
        }
    }

    pub(crate) fn insert(
        &self,
        client_id: &str,
        claims: ClientMetadataDocumentClaims,
        expires: Instant,
        now: Instant,
    ) {
        let mut entries = self.lock();
        if entries.len() >= self.capacity && !entries.contains_key(client_id) {
            entries.retain(|_, entry| entry.expires > now);
            if entries.len() >= self.capacity
                && let Some(soonest) = entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(key, _)| key.clone())
            {
                entries.remove(&soonest);
            }
        }
        entries.insert(client_id.to_owned(), CachedDocument { claims, expires });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, CachedDocument>> {
        // A panic while holding the lock cannot leave an entry half-written.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    const BOUNDS: LifetimeBounds = LifetimeBounds {
        min: Duration::from_secs(300),
        max: Duration::from_secs(3_600),
    };

    fn headers(values: &[(reqwest::header::HeaderName, &str)]) -> HeaderMap {
        values
            .iter()
            .map(|(name, value)| (name.clone(), HeaderValue::from_str(value).unwrap()))
            .collect()
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn should_honour_the_document_cache_lifetime_within_the_bounds() {
        let now = at(1_000_000);
        for (values, expected) in [
            (vec![(CACHE_CONTROL, "max-age=900")], 900),
            (vec![(CACHE_CONTROL, "public, MAX-AGE=\"1200\"")], 1_200),
            (vec![(CACHE_CONTROL, "max-age=60")], 300),
            (vec![(CACHE_CONTROL, "max-age=86400")], 3_600),
            (vec![(CACHE_CONTROL, "max-age=900, no-store")], 300),
            (vec![(CACHE_CONTROL, "no-cache")], 300),
            (vec![(CACHE_CONTROL, "max-age=abc")], 300),
            (
                vec![
                    (CACHE_CONTROL, "max-age=600"),
                    (EXPIRES, "Thu, 01 Jan 1970 23:00:00 GMT"),
                ],
                600,
            ),
            (vec![], 300),
        ] {
            assert_eq!(
                Duration::from_secs(expected),
                BOUNDS.lifetime(&headers(&values), now),
                "{values:?}"
            );
        }
    }

    #[test]
    fn should_derive_the_lifetime_from_expires_relative_to_the_response_date() {
        let date = "Mon, 12 Jan 1970 13:46:40 GMT"; // 1_000_000 seconds
        for (expires, expected) in [
            ("Mon, 12 Jan 1970 14:06:40 GMT", 1_200),
            ("Mon, 12 Jan 1970 13:47:40 GMT", 300),
            ("Tue, 13 Jan 1970 13:46:40 GMT", 3_600),
            ("Thu, 01 Jan 1970 00:00:00 GMT", 300),
            ("not a date", 300),
        ] {
            assert_eq!(
                Duration::from_secs(expected),
                BOUNDS.lifetime(&headers(&[(EXPIRES, expires), (DATE, date)]), at(5)),
                "{expires}"
            );
        }
        assert_eq!(
            Duration::from_secs(1_200),
            BOUNDS.lifetime(
                &headers(&[(EXPIRES, "Mon, 12 Jan 1970 14:06:40 GMT")]),
                at(1_000_000)
            )
        );
    }

    #[test]
    fn should_never_serve_an_expired_entry_and_stay_within_capacity() {
        let cache = DocumentCache::new(2);
        let now = Instant::now();
        let claims = ClientMetadataDocumentClaims::default();

        cache.insert("a", claims.clone(), now + Duration::from_secs(10), now);
        assert_eq!(Some(claims.clone()), cache.get("a", now));
        assert_eq!(None, cache.get("a", now + Duration::from_secs(10)));
        assert_eq!(None, cache.get("a", now));

        cache.insert("a", claims.clone(), now + Duration::from_secs(30), now);
        cache.insert("b", claims.clone(), now + Duration::from_secs(20), now);
        cache.insert("c", claims.clone(), now + Duration::from_secs(40), now);
        assert_eq!(Some(claims.clone()), cache.get("a", now));
        assert_eq!(
            None,
            cache.get("b", now),
            "the entry expiring first is evicted"
        );
        assert_eq!(Some(claims), cache.get("c", now));
    }
}
