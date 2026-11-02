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

/// A page answer that asks the crawler for less traffic: a 429, or a 503 the
/// shop sent with a wait request, the two statuses RFC 9110 pairs with
/// `Retry-After`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RateLimitSignal {
    pub status: u16,
    /// The shop's `Retry-After`, when it sent a usable one.
    pub retry_after: Option<Duration>,
}

/// Headers whose value `0` says the client's request quota is spent: the
/// IETF draft's `RateLimit-Remaining` and its common `X-` predecessor.
const QUOTA_REMAINING_HEADERS: [&str; 2] = ["ratelimit-remaining", "x-ratelimit-remaining"];

impl RateLimitSignal {
    /// The rate-limit answer a page with `status` and the response `headers`
    /// is. `headers` is `None` for a status Spider synthesized for a page it
    /// could not fetch, which is never the shop's answer. A 503 also means
    /// overload or maintenance, so it is a rate-limit answer only with a wait
    /// request: a `Retry-After` header, usable or not, or a spent quota.
    pub fn from_page(status: u16, headers: Option<&HeaderMap>) -> Option<Self> {
        let headers = headers?;
        let asks_to_wait = match status {
            429 => true,
            503 => headers.contains_key(RETRY_AFTER) || quota_spent(headers),
            _ => false,
        };
        asks_to_wait.then(|| Self {
            status,
            retry_after: headers.get(RETRY_AFTER).and_then(parse_retry_after),
        })
    }
}

fn quota_spent(headers: &HeaderMap) -> bool {
    QUOTA_REMAINING_HEADERS.iter().any(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == "0")
    })
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

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    fn signal(status: u16, retry_after: Option<Duration>) -> Option<RateLimitSignal> {
        Some(RateLimitSignal {
            status,
            retry_after,
        })
    }

    #[test]
    fn should_signal_a_429_the_shop_answered() {
        assert_eq!(
            RateLimitSignal::from_page(429, Some(&HeaderMap::new())),
            signal(429, None)
        );
    }

    #[test]
    fn should_not_signal_other_statuses_even_with_a_retry_after() {
        let headers = headers_with_retry_after("60");
        for status in [200, 301, 403, 404, 500, 502, 504] {
            assert_eq!(
                RateLimitSignal::from_page(status, Some(&headers)),
                None,
                "{status}"
            );
        }
    }

    /// Spider builds a page without headers for a request that got no answer,
    /// and gives it a synthetic status such as 503 for an unreachable request.
    #[test]
    fn should_not_signal_a_status_spider_synthesized() {
        assert_eq!(RateLimitSignal::from_page(503, None), None);
        assert_eq!(RateLimitSignal::from_page(429, None), None);
    }

    #[test]
    fn should_not_signal_a_plain_503_without_a_wait_request() {
        let maintenance = headers(&[("content-type", "text/html"), ("server", "nginx")]);
        assert_eq!(RateLimitSignal::from_page(503, Some(&maintenance)), None);

        let quota_left = headers(&[("x-ratelimit-remaining", "12")]);
        assert_eq!(RateLimitSignal::from_page(503, Some(&quota_left)), None);
    }

    #[test]
    fn should_signal_a_503_with_a_retry_after_or_a_spent_quota() {
        let headers_with_wait = headers_with_retry_after("120");
        assert_eq!(
            RateLimitSignal::from_page(503, Some(&headers_with_wait)),
            signal(503, Some(Duration::from_secs(120)))
        );
        for name in ["ratelimit-remaining", "x-ratelimit-remaining"] {
            let mut spent = HeaderMap::new();
            spent.insert(name, HeaderValue::from_static(" 0"));
            assert_eq!(
                RateLimitSignal::from_page(503, Some(&spent)),
                signal(503, None),
                "{name}"
            );
        }
    }

    #[test]
    fn should_carry_a_usable_retry_after_only() {
        let headers = headers_with_retry_after("1800");
        assert_eq!(
            RateLimitSignal::from_page(429, Some(&headers)),
            signal(429, Some(Duration::from_secs(1800)))
        );

        let headers = headers_with_retry_after("soon");
        assert_eq!(
            RateLimitSignal::from_page(503, Some(&headers)),
            signal(503, None)
        );
    }
}
