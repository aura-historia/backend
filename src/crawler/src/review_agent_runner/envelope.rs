//! The result envelope `claude -p --output-format json` prints when a process ends, and what it
//! means for the runner. Every key is optional and read with a default, so a CLI that adds,
//! renames or drops a key degrades to a failed process instead of a crash; a value of the wrong
//! type counts as missing. The envelope's texts (`result`, `errors`) are read only to recognise a
//! usage limit and are never logged or stored: they can carry what the agent read.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time};

/// How long the runner pauses after a usage limit whose reset time it cannot read.
pub const UNREADABLE_RESET_PAUSE: Duration = Duration::hours(1);
/// The longest pause a reported reset time sets; a later one is read as a misreading.
const MAX_PAUSE: Duration = Duration::days(8);
/// The longest `subtype` or `terminal_reason` the runner records.
const MAX_REASON_CHARS: usize = 64;

/// The token counts of a process, summed over its models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

impl TokenUsage {
    pub fn add(&mut self, other: TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
    }
}

/// What the runner reads from one envelope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultEnvelope {
    pub subtype: Option<String>,
    pub is_error: bool,
    pub terminal_reason: Option<String>,
    pub api_error_status: Option<u16>,
    /// The CLI's typed kind of the API error that ended the turn, such as `usage_limit_reached`.
    pub api_error: Option<String>,
    texts: Vec<String>,
    pub structured_output: Option<Value>,
    pub usage: TokenUsage,
    /// The models the process used, by the CLI's `modelUsage`.
    pub models: Vec<String>,
    pub total_cost_usd: f64,
    pub num_turns: u64,
    pub duration_ms: u64,
}

impl ResultEnvelope {
    /// The envelope on a process's stdout: the one JSON object it prints, or the last line that
    /// is one. `None` when there is none.
    pub fn parse(stdout: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(stdout);
        let object = serde_json::from_str::<Value>(text.trim())
            .ok()
            .filter(Value::is_object)
            .or_else(|| {
                text.lines()
                    .rev()
                    .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
                    .find(|value| value.get("type").and_then(Value::as_str) == Some("result"))
            })?;
        Some(Self::of(&object))
    }

    fn of(envelope: &Value) -> Self {
        let text = |key: &str| envelope.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| envelope.get(key).and_then(Value::as_u64).unwrap_or(0);
        let model_usage = envelope
            .get("modelUsage")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut texts: Vec<String> = text("result").into_iter().collect();
        texts.extend(
            envelope
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        Self {
            subtype: text("subtype").map(|reason| bounded_reason(&reason)),
            is_error: envelope
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            terminal_reason: text("terminal_reason").map(|reason| bounded_reason(&reason)),
            api_error_status: envelope
                .get("api_error_status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
            api_error: text("api_error").map(|kind| bounded_reason(&kind)),
            texts,
            structured_output: envelope
                .get("structured_output")
                .filter(|output| !output.is_null())
                .cloned(),
            usage: token_usage(envelope.get("usage")),
            models: model_usage
                .keys()
                .map(|model| bounded_reason(model))
                .collect(),
            total_cost_usd: envelope
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .filter(|cost| cost.is_finite() && *cost >= 0.0)
                .unwrap_or(0.0),
            num_turns: number("num_turns"),
            duration_ms: number("duration_ms"),
        }
    }
}

fn token_usage(usage: Option<&Value>) -> TokenUsage {
    let count = |key: &str| {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    TokenUsage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_read_input_tokens: count("cache_read_input_tokens"),
        cache_creation_input_tokens: count("cache_creation_input_tokens"),
    }
}

/// A `subtype`, `terminal_reason` or model name as the runner records it: an identifier of the
/// CLI, never free text.
fn bounded_reason(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "_-.[]:".contains(*character))
        .take(MAX_REASON_CHARS)
        .collect()
}

/// A limit of the process itself that ended it before it decided anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProcessLimit {
    /// `--max-turns`.
    Turns,
    /// `--max-budget-usd`.
    Budget,
    /// The model's context window.
    Context,
}

/// Which plan limit of the CLI's login a process ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UsageLimitKind {
    /// The rolling five-hour session window.
    Session,
    /// The weekly window.
    Weekly,
    /// A model's own weekly window, such as Opus.
    Model,
    /// A spend limit or the account's usage credits.
    Spend,
}

/// A usage limit of the CLI's login, and when it resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageLimit {
    pub kind: UsageLimitKind,
    /// The reset time the CLI reported, `None` when it named none the runner can read.
    pub resets_at: Option<OffsetDateTime>,
}

impl UsageLimit {
    /// Until when the runner starts no agent process: the reported reset time, or a short pause
    /// when it cannot be read, never more than [`MAX_PAUSE`] ahead.
    pub fn paused_until(&self, now: OffsetDateTime) -> OffsetDateTime {
        self.resets_at
            .filter(|resets_at| *resets_at > now)
            .unwrap_or(now + UNREADABLE_RESET_PAUSE)
            .min(now + MAX_PAUSE)
    }
}

/// How a process ended, for the runner.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvelopeOutcome {
    /// The process finished its turn with a structured report, not yet checked against the role's
    /// report type.
    Reported(Value),
    /// The process hit its own turn, budget or context limit before it decided.
    LimitReached(ProcessLimit),
    /// The CLI's login ran into a plan limit.
    UsageLimit(UsageLimit),
    /// Any other end without a report, named by the envelope's `subtype` or `terminal_reason`.
    Failed,
}

impl ResultEnvelope {
    /// What the envelope means, with `now` for a reset time that names no date.
    pub fn outcome(&self, now: OffsetDateTime) -> EnvelopeOutcome {
        if let Some(limit) = self.usage_limit(now) {
            return EnvelopeOutcome::UsageLimit(limit);
        }
        if let Some(limit) = self.process_limit() {
            return EnvelopeOutcome::LimitReached(limit);
        }
        match (
            &self.structured_output,
            self.is_error,
            self.subtype.as_deref(),
        ) {
            (Some(report), false, None | Some("success")) => {
                EnvelopeOutcome::Reported(report.clone())
            }
            _ => EnvelopeOutcome::Failed,
        }
    }

    fn process_limit(&self) -> Option<ProcessLimit> {
        match (self.subtype.as_deref(), self.terminal_reason.as_deref()) {
            (Some("error_max_turns"), _) | (_, Some("max_turns")) => Some(ProcessLimit::Turns),
            (Some("error_max_budget_usd"), _) | (_, Some("budget_exhausted")) => {
                Some(ProcessLimit::Budget)
            }
            (_, Some("prompt_too_long" | "blocking_limit")) => Some(ProcessLimit::Context),
            _ => None,
        }
    }

    /// The usage limit an error envelope reports: the CLI's typed `usage_limit_reached`, or its
    /// limit message (`You've hit your 5-hour limit · resets 3pm (UTC)`).
    fn usage_limit(&self, now: OffsetDateTime) -> Option<UsageLimit> {
        if !self.is_error {
            return None;
        }
        let message = self
            .texts
            .iter()
            .find_map(|text| limit_message_kind(text).map(|kind| (kind, text)));
        let typed = self.api_error.as_deref() == Some("usage_limit_reached");
        let (kind, text) = match message {
            Some((kind, text)) => (kind, Some(text)),
            None if typed => (UsageLimitKind::Session, None),
            None => return None,
        };
        Some(UsageLimit {
            kind,
            resets_at: text.and_then(|text| reset_time(text, now)),
        })
    }
}

/// The kind of limit a CLI limit message names, such as `You've hit your weekly limit`.
fn limit_message_kind(text: &str) -> Option<UsageLimitKind> {
    static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(?:you've|you have) (?:hit|reached) your (?P<limit>[^·.\n]+?)(?:\s*[·.]|\s*$)",
        )
        .unwrap_or_else(|error| panic!("limit message pattern: {error}"))
    });
    if text.contains("You're out of usage credits") {
        return Some(UsageLimitKind::Spend);
    }
    let limit = LIMIT
        .captures(text)?
        .name("limit")?
        .as_str()
        .to_ascii_lowercase();
    Some(if limit.contains("5-hour") || limit.contains("session") {
        UsageLimitKind::Session
    } else if limit.starts_with("weekly") {
        UsageLimitKind::Weekly
    } else if ["opus", "sonnet", "haiku", "fable"]
        .iter()
        .any(|model| limit.starts_with(model))
    {
        UsageLimitKind::Model
    } else {
        UsageLimitKind::Spend
    })
}

/// The reset time of a limit message, `resets 3pm (UTC)`, `resets 9:30am (UTC)`,
/// `resets Nov 3, 9am (UTC)` or `resets Jan 3, 2027, 9am (UTC)`, as the CLI writes it in the
/// process's time zone, which the runner sets to UTC. A time without a date is the next such time
/// after `now`; a time in any other zone is not read.
fn reset_time(text: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    static RESET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"resets (?:(?P<month>[A-Z][a-z]{2}) (?P<day>\d{1,2}), (?:(?P<year>\d{4}), )?)?(?P<hour>\d{1,2})(?::(?P<minute>\d{2}))?\s?(?P<meridiem>[ap]m) \((?P<zone>[^)]+)\)",
        )
        .unwrap_or_else(|error| panic!("reset time pattern: {error}"))
    });
    let reset = RESET.captures(text)?;
    if !matches!(&reset["zone"], "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Z") {
        return None;
    }
    let hour12: u8 = reset["hour"]
        .parse()
        .ok()
        .filter(|hour| (1..=12).contains(hour))?;
    let minute: u8 = reset
        .name("minute")
        .map_or(Some(0), |minute| minute.as_str().parse().ok())?;
    let hour = (hour12 % 12) + if &reset["meridiem"] == "pm" { 12 } else { 0 };
    let time = Time::from_hms(hour, minute, 0).ok()?;
    let Some(month) = reset.name("month") else {
        let today = PrimitiveDateTime::new(now.date(), time).assume_utc();
        return Some(if today > now {
            today
        } else {
            today + Duration::days(1)
        });
    };
    let month = month_of(month.as_str())?;
    let day: u8 = reset["day"].parse().ok()?;
    let year = reset
        .name("year")
        .map_or(Some(now.year()), |year| year.as_str().parse().ok())?;
    let date = Date::from_calendar_date(year, month, day).ok()?;
    Some(PrimitiveDateTime::new(date, time).assume_utc())
}

fn month_of(name: &str) -> Option<Month> {
    const MONTHS: [(&str, Month); 12] = [
        ("Jan", Month::January),
        ("Feb", Month::February),
        ("Mar", Month::March),
        ("Apr", Month::April),
        ("May", Month::May),
        ("Jun", Month::June),
        ("Jul", Month::July),
        ("Aug", Month::August),
        ("Sep", Month::September),
        ("Oct", Month::October),
        ("Nov", Month::November),
        ("Dec", Month::December),
    ];
    MONTHS
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, month)| *month)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-10-28 13:20 UTC);

    fn envelope(value: Value) -> ResultEnvelope {
        ResultEnvelope::parse(value.to_string().as_bytes())
            .unwrap_or_else(|| panic!("no envelope in {value}"))
    }

    #[test]
    fn should_read_a_finished_report_with_its_usage_and_cost() {
        let read = envelope(json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "num_turns": 17,
            "duration_ms": 93_000,
            "result": "Done.",
            "structured_output": { "review_id": "cr_1" },
            "total_cost_usd": 1.25,
            "usage": {
                "input_tokens": 1200,
                "output_tokens": 800,
                "cache_read_input_tokens": 50_000,
                "cache_creation_input_tokens": 9_000,
                "service_tier": "standard",
            },
            "modelUsage": { "claude-opus-5-5": { "costUSD": 1.25 } },
            "terminal_reason": "completed",
        }));

        assert_eq!(
            read.outcome(NOW),
            EnvelopeOutcome::Reported(json!({ "review_id": "cr_1" }))
        );
        assert_eq!(
            read.usage,
            TokenUsage {
                input_tokens: 1200,
                output_tokens: 800,
                cache_read_input_tokens: 50_000,
                cache_creation_input_tokens: 9_000,
            }
        );
        assert_eq!(
            (read.total_cost_usd, read.num_turns, read.duration_ms),
            (1.25, 17, 93_000)
        );
        assert_eq!(read.models, ["claude-opus-5-5"]);
    }

    #[test]
    fn should_default_every_missing_or_mistyped_key() {
        let read = envelope(json!({
            "type": "result",
            "is_error": "yes",
            "total_cost_usd": "1.0",
            "usage": [],
            "api_error_status": null,
        }));

        assert_eq!(read, ResultEnvelope::default());
        assert_eq!(read.outcome(NOW), EnvelopeOutcome::Failed);
        assert_eq!(ResultEnvelope::parse(b"not json\n"), None);
        assert_eq!(ResultEnvelope::parse(b""), None);
    }

    #[test]
    fn should_find_the_result_line_after_other_output() {
        let read = ResultEnvelope::parse(
            b"warning: something\n{\"type\":\"result\",\"subtype\":\"success\",\"structured_output\":{\"a\":1}}\n",
        );

        assert_eq!(
            read.map(|read| read.outcome(NOW)),
            Some(EnvelopeOutcome::Reported(json!({ "a": 1 })))
        );
    }

    #[test]
    fn should_end_without_a_decision_on_the_process_s_own_limits() {
        for (envelope_json, limit) in [
            (
                json!({ "subtype": "error_max_turns", "is_error": true, "terminal_reason": "max_turns" }),
                ProcessLimit::Turns,
            ),
            (
                json!({ "subtype": "error_max_budget_usd", "is_error": true, "terminal_reason": "budget_exhausted" }),
                ProcessLimit::Budget,
            ),
            (
                json!({ "subtype": "success", "is_error": true, "terminal_reason": "prompt_too_long" }),
                ProcessLimit::Context,
            ),
        ] {
            assert_eq!(
                envelope(envelope_json.clone()).outcome(NOW),
                EnvelopeOutcome::LimitReached(limit),
                "{envelope_json}"
            );
        }
        assert_eq!(
            envelope(json!({ "subtype": "error_max_structured_output_retries", "is_error": true }))
                .outcome(NOW),
            EnvelopeOutcome::Failed
        );
        assert_eq!(
            envelope(json!({ "subtype": "success", "is_error": true, "result": "API Error: 500", "structured_output": {} }))
                .outcome(NOW),
            EnvelopeOutcome::Failed
        );
    }

    #[test]
    fn should_read_the_usage_limit_and_its_reset_time() {
        let limit = |result: &str| {
            envelope(json!({
                "subtype": "success",
                "is_error": true,
                "api_error_status": 429,
                "result": result,
            }))
            .outcome(NOW)
        };

        assert_eq!(
            limit("You've hit your 5-hour limit · resets 3pm (UTC)"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: Some(datetime!(2026-10-28 15:00 UTC)),
            })
        );
        assert_eq!(
            limit("You've hit your session limit · resets 1:15pm (UTC)"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: Some(datetime!(2026-10-29 13:15 UTC)),
            })
        );
        assert_eq!(
            limit("You've hit your weekly limit · resets Nov 3, 9am (UTC) · progress saved"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Weekly,
                resets_at: Some(datetime!(2026-11-03 09:00 UTC)),
            })
        );
        assert_eq!(
            limit("You've hit your Opus limit · resets Jan 2, 2027, 12am (UTC)"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Model,
                resets_at: Some(datetime!(2027-01-02 00:00 UTC)),
            })
        );
        assert_eq!(
            limit("API Error: You've hit your Sonnet limit · resets 11:45pm (UTC)"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Model,
                resets_at: Some(datetime!(2026-10-28 23:45 UTC)),
            })
        );
        assert_eq!(
            limit("You've hit your 5-hour limit · resets 3pm (Europe/Berlin)"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: None,
            })
        );
        assert_eq!(
            limit("You've hit your org's monthly spend limit · ask your admin"),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Spend,
                resets_at: None,
            })
        );
        assert_eq!(
            envelope(
                json!({ "is_error": true, "api_error": "usage_limit_reached", "errors": ["limit"] })
            )
            .outcome(NOW),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: None,
            })
        );
        assert_eq!(
            envelope(json!({ "is_error": true, "api_error_status": 429, "result": "API Error: 429 overloaded" }))
                .outcome(NOW),
            EnvelopeOutcome::Failed
        );
    }

    #[test]
    fn should_pause_until_the_reset_or_briefly_when_it_cannot_be_read() {
        let read = |resets_at| UsageLimit {
            kind: UsageLimitKind::Weekly,
            resets_at,
        };

        assert_eq!(
            read(Some(datetime!(2026-10-30 09:00 UTC))).paused_until(NOW),
            datetime!(2026-10-30 09:00 UTC)
        );
        assert_eq!(read(None).paused_until(NOW), NOW + Duration::hours(1));
        assert_eq!(
            read(Some(datetime!(2026-10-28 13:00 UTC))).paused_until(NOW),
            NOW + Duration::hours(1)
        );
        assert_eq!(
            read(Some(datetime!(2027-10-28 13:00 UTC))).paused_until(NOW),
            NOW + Duration::days(8)
        );
    }

    #[test]
    fn should_keep_only_identifier_characters_of_a_reason() {
        let read = envelope(json!({
            "subtype": "error_during_execution\n<script>",
            "terminal_reason": "x".repeat(200),
        }));

        assert_eq!(
            read.subtype.as_deref(),
            Some("error_during_executionscript")
        );
        assert_eq!(read.terminal_reason.map(|reason| reason.len()), Some(64));
    }
}
