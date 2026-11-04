//! What the status of each page Spider delivers tells the crawl: the
//! per-class counts the crawl diagnostics carry, and the answers that ask
//! the crawler for less traffic.

use std::time::Duration;

use spider::reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::network::policy::{MAX_RETRY_AFTER, parse_retry_after};

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
    /// The wait the shop asked for: its usable `Retry-After`, else the reset of its `RateLimit`
    /// policies, capped like every other wait.
    pub retry_after: Option<Duration>,
}

/// Headers whose value `0` says the client's request quota is spent: the
/// IETF draft's earlier `RateLimit-Remaining` and its common `X-` predecessor.
const QUOTA_REMAINING_HEADERS: [&str; 2] = ["ratelimit-remaining", "x-ratelimit-remaining"];

/// The IETF draft's structured `RateLimit` field: a list of the client's quota
/// policies, each with the quota units it has left as its `r` parameter, such
/// as `"default";r=0;t=30`.
const QUOTA_POLICIES_HEADER: &str = "ratelimit";

/// The draft's earlier reset header: the seconds until the quota resets.
const RESET_HEADER: &str = "ratelimit-reset";

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
            retry_after: headers
                .get(RETRY_AFTER)
                .and_then(parse_retry_after)
                .or_else(|| quota_reset(headers)),
        })
    }
}

/// Whether `headers` say a quota of the client is spent, in either header
/// style. A malformed field or a value that does not parse says nothing.
fn quota_spent(headers: &HeaderMap) -> bool {
    let remaining_spent = QUOTA_REMAINING_HEADERS.iter().any(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == "0")
    });
    remaining_spent || policies_field(headers).is_some_and(|field| names_a_spent_policy(&field))
}

/// The wait until the client's quota resets, when the shop gave no usable `Retry-After`: the `t`
/// parameter of the `RateLimit` field's policies, the longest of the spent ones' (`r=0`), none when
/// a spent one has no `t`, or, when none is spent, the longest of all; else the delta seconds of the draft's earlier `RateLimit-Reset`.
/// `X-RateLimit-Reset` is not read: shops send it as an epoch or as seconds. A malformed field
/// says nothing. The wait is capped at [`MAX_RETRY_AFTER`].
fn quota_reset(headers: &HeaderMap) -> Option<Duration> {
    let seconds = policies_field(headers)
        .and_then(|field| policies_reset_seconds(&field))
        .or_else(|| {
            let value = headers.get(RESET_HEADER)?.to_str().ok()?.trim();
            all_digits(value).then(|| value.parse().ok()).flatten()
        })?;
    Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER))
}

/// The reset seconds a `RateLimit` field value names, see [`quota_reset`].
fn policies_reset_seconds(field: &str) -> Option<u64> {
    let members = split_outside_strings(field, b',')?;
    if members.iter().any(|member| member.trim().is_empty()) {
        return None;
    }
    let policies: Vec<(bool, Option<u64>)> = members
        .into_iter()
        .map(|member| {
            (
                remaining_quota(member) == Some(0),
                parameter_seconds(member, "t"),
            )
        })
        .collect();
    // Another policy's reset says nothing about when a spent quota resets, so a spent policy
    // without `t` leaves the wait unknown.
    let spent: Vec<Option<u64>> = policies
        .iter()
        .filter(|(spent, _)| *spent)
        .map(|(_, reset)| *reset)
        .collect();
    match spent.is_empty() {
        true => policies.iter().filter_map(|(_, reset)| *reset).max(),
        false => spent
            .into_iter()
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max(),
    }
}

/// The last occurrence of parameter `name` of a `RateLimit` member, when it is a non-negative
/// integer.
fn parameter_seconds(member: &str, name: &str) -> Option<u64> {
    let (_, value) = split_outside_strings(member, b';')?
        .into_iter()
        .skip(1)
        .map(|parameter| {
            let parameter = parameter.trim();
            parameter.split_once('=').unwrap_or((parameter, ""))
        })
        .rfind(|(key, _)| *key == name)?;
    all_digits(value).then(|| value.parse().ok()).flatten()
}

fn all_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The `RateLimit` field value with its field lines combined, as a
/// structured-field List is parsed; none when a line is not text.
fn policies_field(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(QUOTA_POLICIES_HEADER)
        .iter()
        .map(|line| line.to_str().ok())
        .collect::<Option<Vec<_>>>()
        .map(|lines| lines.join(","))
}

/// Whether a `RateLimit` field value lists a policy with no quota left: a
/// member whose `r` parameter, after its policy name, is `0`. A value with an
/// unterminated string or an empty member is malformed, and the draft has a
/// malformed field ignored.
fn names_a_spent_policy(field: &str) -> bool {
    let Some(members) = split_outside_strings(field, b',') else {
        return false;
    };
    members.iter().all(|member| !member.trim().is_empty())
        && members
            .into_iter()
            .any(|member| remaining_quota(member) == Some(0))
}

/// The `r` parameter of a `RateLimit` member, when it is a non-negative
/// integer: its last occurrence, since a repeated structured-field parameter
/// overwrites the earlier ones.
fn remaining_quota(member: &str) -> Option<u64> {
    parameter_seconds(member, "r")
}

/// `value` split at each ASCII `separator` outside a structured-field string,
/// which may quote either separator in a policy name or a parameter; none when
/// a string is left unterminated.
fn split_outside_strings(value: &str, separator: u8) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            _ if byte == separator && !quoted => {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    (!quoted).then_some(parts)
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

    /// Headers with one `RateLimit` field line per entry of `lines`.
    fn ratelimit_field(lines: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append("ratelimit", HeaderValue::from_static(line));
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

        for field_lines in [
            &[r#""default";r=12;t=30"#][..],
            &[r#""burst";q=0;r=5;t=1"#],
            &[r#""r=0";r=3"#],
            &[r#""default";r=3;note="x;r=0;y""#],
            &[r#""default";r=5;note="a, b;r=0;c""#],
            &["r=0"],
            &[r#""default";t=0, "daily";r=1"#],
            &[r#""default";r=0;r=5"#],
            &[r#""default";r=zero"#],
            &[r#""default";r=+0"#],
        ] {
            assert_eq!(
                RateLimitSignal::from_page(503, Some(&ratelimit_field(field_lines))),
                None,
                "{field_lines:?}"
            );
        }
    }

    /// The draft has a malformed `RateLimit` field ignored, a spent policy
    /// in it included.
    #[test]
    fn should_not_signal_a_503_whose_ratelimit_field_is_malformed() {
        for field_lines in [
            &[r#""default";r=0, "daily"#][..],
            &[r#""default";r=0,"#],
            &[r#""default";r=0"#, r#""daily"#],
        ] {
            assert_eq!(
                RateLimitSignal::from_page(503, Some(&ratelimit_field(field_lines))),
                None,
                "{field_lines:?}"
            );
        }
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

    /// The IETF draft's structured field lists every quota policy; one spent
    /// policy is enough, whichever field line or member carries it.
    #[test]
    fn should_signal_a_503_whose_ratelimit_field_lists_a_spent_policy() {
        let wait = |secs| Some(Duration::from_secs(secs));
        for (field_lines, expected) in [
            (&[r#""default";r=0;t=30"#][..], wait(30)),
            (&["default;r=0"], None),
            (
                &[r#""per-minute";r=42;t=10, "daily, all";r=0;t=3600"#],
                wait(3600),
            ),
            (&[r#""a\";b";r=0"#], None),
            (&[r#""default";r=5;r=0"#], None),
            (&[r#""default";r=12;t=30"#, r#""daily";r=0"#], None),
        ] {
            let mut spent = ratelimit_field(field_lines);
            spent.insert(
                "ratelimit-policy",
                HeaderValue::from_static(r#""default";q=100;w=60"#),
            );
            assert_eq!(
                RateLimitSignal::from_page(503, Some(&spent)),
                signal(503, expected),
                "{field_lines:?}"
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

    /// Both header styles carry the wait when the shop sent no `Retry-After`: the structured
    /// field's `t` and the draft's earlier `RateLimit-Reset`, capped like `Retry-After`.
    #[test]
    fn should_wait_for_the_ratelimit_reset_without_a_retry_after() {
        let seconds = |secs| signal(429, Some(Duration::from_secs(secs)));
        for (field_lines, expected) in [
            (&[r#""default";r=0;t=30"#][..], 30),
            (&[r#""per-minute";r=42;t=10, "daily";r=0;t=3600"#], 3600),
            (&[r#""a";r=0;t=5"#, r#""b";r=0;t=45"#], 45),
            (&[r#""default";r=3;t=20"#], 20),
            (&[r#""default";r=0;t=999999999"#], 86_400),
        ] {
            assert_eq!(
                RateLimitSignal::from_page(429, Some(&ratelimit_field(field_lines))),
                seconds(expected),
                "{field_lines:?}"
            );
        }
        let mut reset = HeaderMap::new();
        reset.insert("ratelimit-reset", HeaderValue::from_static(" 17 "));
        assert_eq!(RateLimitSignal::from_page(429, Some(&reset)), seconds(17));
        reset.insert("ratelimit-reset", HeaderValue::from_static("9999999"));
        assert_eq!(
            RateLimitSignal::from_page(429, Some(&reset)),
            seconds(86_400)
        );

        let mut spent = ratelimit_field(&[r#""default";r=0;t=30"#]);
        assert_eq!(
            RateLimitSignal::from_page(503, Some(&spent)),
            signal(503, Some(Duration::from_secs(30)))
        );
        spent.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(
            RateLimitSignal::from_page(503, Some(&spent)),
            signal(503, Some(Duration::from_secs(120)))
        );
    }

    #[test]
    fn should_not_wait_for_a_reset_that_is_not_usable() {
        for field_lines in [
            &[r#""default";r=0;t=soon"#][..],
            &[r#""default";r=0;t=-3"#],
            &[r#""default";r=0"#],
            &[r#""default";r=0;t=30, "#],
            &[r#""default";r=0;t=30, "daily"#],
            &[r#""per-minute";r=42;t=10, "daily";r=0"#],
        ] {
            let signal = RateLimitSignal::from_page(429, Some(&ratelimit_field(field_lines)));
            assert_eq!(signal.and_then(|s| s.retry_after), None, "{field_lines:?}");
        }
        let mut reset = HeaderMap::new();
        for value in ["soon", "-5", "", "1.5"] {
            reset.insert("ratelimit-reset", HeaderValue::from_str(value).unwrap());
            reset.insert("x-ratelimit-reset", HeaderValue::from_static("1793704838"));
            assert_eq!(
                RateLimitSignal::from_page(429, Some(&reset)),
                signal(429, None),
                "{value}"
            );
        }
    }
}
