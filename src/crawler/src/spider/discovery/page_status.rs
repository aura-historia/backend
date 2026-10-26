//! What the status of each page Spider delivers tells the crawl: the
//! per-class counts the crawl diagnostics carry, and the answers that ask
//! the crawler for less traffic.

use std::time::Duration;

use spider::reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::network::policy::parse_retry_after;

/// Pages Spider delivered, counted by the status class of their final answer
/// (after Spider's own retries). Bounded by the crawl page budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    /// 2xx.
    pub success: usize,
    /// 3xx: a redirect Spider did not follow to a page.
    pub redirect: usize,
    /// 4xx other than 429.
    pub client_error: usize,
    /// 429.
    pub rate_limited: usize,
    /// 5xx, Spider's synthetic transport statuses included.
    pub server_error: usize,
    /// Anything outside 2xx-5xx.
    pub other: usize,
}

impl StatusCounts {
    pub fn count(&mut self, status: u16) {
        let bucket = match status {
            200..=299 => &mut self.success,
            300..=399 => &mut self.redirect,
            429 => &mut self.rate_limited,
            400..=499 => &mut self.client_error,
            500..=599 => &mut self.server_error,
            _ => &mut self.other,
        };
        *bucket += 1;
    }
}

/// A page answer that asks the crawler for less traffic: 429, or 503, the
/// two statuses RFC 9110 pairs with `Retry-After` as a wait request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitSignal {
    pub status: u16,
    /// The shop's `Retry-After`, when it sent a usable one.
    pub retry_after: Option<Duration>,
}

impl RateLimitSignal {
    pub fn from_page(status: u16, headers: Option<&HeaderMap>) -> Option<Self> {
        if !matches!(status, 429 | 503) {
            return None;
        }
        let retry_after = headers
            .and_then(|headers| headers.get(RETRY_AFTER))
            .and_then(parse_retry_after);
        Some(Self {
            status,
            retry_after,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spider::reqwest::header::HeaderValue;

    fn headers_with_retry_after(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn should_count_each_status_in_its_class_and_429_apart() {
        let mut counts = StatusCounts::default();
        for status in [200, 204, 301, 404, 403, 429, 429, 500, 503, 599, 100] {
            counts.count(status);
        }

        assert_eq!(
            counts,
            StatusCounts {
                success: 2,
                redirect: 1,
                client_error: 2,
                rate_limited: 2,
                server_error: 3,
                other: 1,
            }
        );
    }

    #[test]
    fn should_signal_only_429_and_503_pages() {
        for status in [200, 301, 403, 404, 500, 502, 504] {
            assert_eq!(RateLimitSignal::from_page(status, None), None, "{status}");
        }
        assert_eq!(
            RateLimitSignal::from_page(429, None),
            Some(RateLimitSignal {
                status: 429,
                retry_after: None,
            })
        );
        assert_eq!(
            RateLimitSignal::from_page(503, None),
            Some(RateLimitSignal {
                status: 503,
                retry_after: None,
            })
        );
    }

    #[test]
    fn should_carry_a_usable_retry_after_only() {
        let headers = headers_with_retry_after("1800");
        assert_eq!(
            RateLimitSignal::from_page(429, Some(&headers)),
            Some(RateLimitSignal {
                status: 429,
                retry_after: Some(Duration::from_secs(1800)),
            })
        );

        let headers = headers_with_retry_after("soon");
        assert_eq!(
            RateLimitSignal::from_page(503, Some(&headers)),
            Some(RateLimitSignal {
                status: 503,
                retry_after: None,
            })
        );
    }
}
