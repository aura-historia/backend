//! Pacing the agents within the login's usage windows before a limit stops them. Every agent
//! process's `stream-json` output carries `rate_limit_event`s whose `unifiedWindows` report how
//! much of the subscription's rolling five-hour and seven-day windows is used, shared with every
//! other use of the same login, such as the people who work with Claude Code on it. After each
//! process the runner reads the last of them and pauses before the agents take more than their
//! share:
//!
//! - five hours: at the configured share of the window, until it resets;
//! - seven days: at a share that grows with the window, from a seventh of the configured share at
//!   its start to the whole share at its end, until the window's elapsed part catches up with the
//!   use, so the agents never spend the week in its first days.
//!
//! A stream without the windows, such as one from a login that is no subscription, sets no pause;
//! the usage limit itself still pauses the runner ([`super::envelope`]).

use super::envelope::{UsageLimit, UsageLimitKind};
use serde_json::Value;
use time::{Duration, OffsetDateTime};

const WEEK: Duration = Duration::days(7);

/// One usage window as the CLI reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageWindow {
    /// The used share of the window, from 0 to 1.
    pub utilization: f64,
    pub resets_at: OffsetDateTime,
}

/// The login's usage windows, as the last `rate_limit_event` of a stream reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageWindows {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
}

impl UsageWindows {
    /// The windows of the last `rate_limit_event` in `stdout` that reports any.
    pub fn parse(stdout: &[u8]) -> Self {
        String::from_utf8_lossy(stdout)
            .lines()
            .rev()
            .filter(|line| line.contains("rate_limit_event"))
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .filter(|event| event.get("type").and_then(Value::as_str) == Some("rate_limit_event"))
            .map(|event| Self::of(&event))
            .find(|windows| windows.five_hour.is_some() || windows.seven_day.is_some())
            .unwrap_or_default()
    }

    fn of(event: &Value) -> Self {
        let windows = event
            .get("rate_limit_info")
            .and_then(|info| info.get("unifiedWindows"));
        let window = |name: &str| {
            let window = windows?.get(name)?;
            let utilization = window
                .get("utilization")
                .and_then(Value::as_f64)
                .filter(|share| share.is_finite() && *share >= 0.0)?;
            let resets_at = window
                .get("resetsAt")
                .and_then(Value::as_i64)
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())?;
            Some(UsageWindow {
                utilization,
                resets_at,
            })
        };
        Self {
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
        }
    }
}

/// The shares of the login's usage windows the agents may take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsagePacing {
    /// The share of the five-hour window, from 0 to 1.
    pub five_hour_share: f64,
    /// The share of the seven-day window at its end, from 0 to 1.
    pub weekly_share: f64,
}

impl UsagePacing {
    /// The pause `windows` call for at `now`, as the usage limit it anticipates; the later one
    /// when both do.
    pub fn pause(&self, windows: &UsageWindows, now: OffsetDateTime) -> Option<UsageLimit> {
        let five_hour = windows
            .five_hour
            .filter(|window| window.resets_at > now && window.utilization >= self.five_hour_share)
            .map(|window| window.resets_at);
        let weekly = windows
            .seven_day
            .and_then(|window| self.weekly_catch_up(window, now));
        let (kind, until) = match (five_hour, weekly) {
            (Some(five_hour), Some(weekly)) if weekly > five_hour => {
                (UsageLimitKind::Weekly, weekly)
            }
            (Some(five_hour), _) => (UsageLimitKind::Session, five_hour),
            (None, Some(weekly)) => (UsageLimitKind::Weekly, weekly),
            (None, None) => return None,
        };
        Some(UsageLimit {
            kind,
            resets_at: Some(until),
            message: None,
        })
    }

    /// When the weekly share allowed by then reaches the window's use, if it has not yet at
    /// `now`, and the window's reset once the use reaches the whole share: the allowed share grows linearly from a seventh of `weekly_share` at the window's
    /// start to `weekly_share` at its end.
    fn weekly_catch_up(&self, window: UsageWindow, now: OffsetDateTime) -> Option<OffsetDateTime> {
        if window.resets_at <= now || self.weekly_share <= 0.0 {
            return None;
        }
        let start = window.resets_at - WEEK;
        let allowed = |at: OffsetDateTime| {
            let elapsed = ((at - start) / WEEK).clamp(0.0, 1.0);
            self.weekly_share * (elapsed + 1.0 / 7.0).min(1.0)
        };
        if window.utilization < allowed(now) {
            return None;
        }
        if window.utilization >= self.weekly_share {
            return Some(window.resets_at);
        }
        let elapsed_needed = window.utilization / self.weekly_share - 1.0 / 7.0;
        Some((start + WEEK * elapsed_needed).clamp(now, window.resets_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-09 12:00 UTC);
    const PACING: UsagePacing = UsagePacing {
        five_hour_share: 0.7,
        weekly_share: 0.7,
    };

    fn event(five_hour: f64, seven_day: f64) -> Value {
        json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed", "rateLimitType": "five_hour",
                "unifiedWindows": {
                    "five_hour": { "utilization": five_hour, "resetsAt": (NOW + Duration::hours(2)).unix_timestamp(), "status": "allowed" },
                    "seven_day": { "utilization": seven_day, "resetsAt": (NOW + Duration::days(3)).unix_timestamp(), "status": "allowed" }
                }
            }
        })
    }

    fn windows(five_hour: f64, seven_day: f64) -> UsageWindows {
        UsageWindows::of(&event(five_hour, seven_day))
    }

    #[test]
    fn should_read_the_windows_of_the_stream_s_last_rate_limit_event_that_reports_them() {
        let stdout = [
            event(0.1, 0.2).to_string(),
            event(0.43, 0.33).to_string(),
            json!({ "type": "rate_limit_event", "rate_limit_info": { "status": "allowed" } })
                .to_string(),
            json!({ "type": "result", "subtype": "success" }).to_string(),
        ]
        .join("\n");

        let read = UsageWindows::parse(stdout.as_bytes());

        assert_eq!(
            read.five_hour,
            Some(UsageWindow {
                utilization: 0.43,
                resets_at: NOW + Duration::hours(2)
            })
        );
        assert_eq!(read.seven_day.map(|window| window.utilization), Some(0.33));
        assert_eq!(UsageWindows::parse(b"no json\n"), UsageWindows::default());
    }

    #[test]
    fn should_pause_at_the_five_hour_share_until_the_window_resets() {
        assert_eq!(PACING.pause(&windows(0.69, 0.0), NOW), None);

        let pause = PACING.pause(&windows(0.7, 0.0), NOW);

        assert_eq!(
            pause,
            Some(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: Some(NOW + Duration::hours(2)),
                message: None
            })
        );
    }

    #[test]
    fn should_pace_the_week_so_its_share_grows_with_the_elapsed_window() {
        // Four of seven days have passed: 0.7 × (4/7 + 1/7) = 0.5 is allowed now.
        assert_eq!(PACING.pause(&windows(0.0, 0.49), NOW), None);

        let pause = PACING
            .pause(&windows(0.0, 0.6), NOW)
            .unwrap_or_else(|| panic!("no pause"));

        // 0.6 is allowed once 0.6 / 0.7 - 1/7 = 5/7 of the week has passed: a day from now.
        assert_eq!(pause.kind, UsageLimitKind::Weekly);
        let until = pause.resets_at.unwrap_or_else(|| panic!("no end"));
        assert!(
            (until - (NOW + Duration::days(1))).abs() < Duration::seconds(1),
            "{until}"
        );
        assert_eq!(
            PACING
                .pause(&windows(0.0, 0.7), NOW)
                .and_then(|pause| pause.resets_at),
            Some(NOW + Duration::days(3)),
            "the whole share pauses until the window resets"
        );
    }

    #[test]
    fn should_take_the_later_pause_when_both_windows_call_for_one() {
        let pause = PACING
            .pause(&windows(0.9, 0.7), NOW)
            .unwrap_or_else(|| panic!("no pause"));

        assert_eq!(pause.kind, UsageLimitKind::Weekly);
        assert_eq!(pause.resets_at, Some(NOW + Duration::days(3)));
    }
}
