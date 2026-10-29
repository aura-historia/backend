//! What a `claude -p --output-format stream-json --verbose` process printed, and what it means for
//! the runner: the result envelope of its last `result` line, and the last `rate_limit_event`,
//! whose `resetsAt` is the CLI's structured reset time of a usage limit that rejected a request.
//! Every other line, what the agent read among them, is dropped once its type is read. Every key
//! is optional and read with a default, so a CLI that adds, renames or drops a key degrades to a
//! failed process instead of a crash; a value of the wrong type counts as missing. The envelope's
//! texts (`result`, `errors`) are read only to recognise a usage limit and are never stored; they
//! can carry what the agent read, so the log gets at most the line of a usage-limit message whose
//! reset time the runner cannot read.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, Weekday};

/// How long the runner pauses after a usage limit whose reset time it cannot read.
pub const UNREADABLE_RESET_PAUSE: Duration = Duration::hours(1);
/// The longest pause a reported reset time sets; a later one is read as a misreading.
const MAX_PAUSE: Duration = Duration::days(8);
/// The longest `subtype` or `terminal_reason` the runner records.
const MAX_REASON_CHARS: usize = 64;
/// The longest part of a usage-limit message the runner logs.
const MAX_LIMIT_MESSAGE_CHARS: usize = 200;
/// The zones a reset time may name; the runner starts the CLI in UTC.
const UTC_ZONES: [&str; 5] = ["UTC", "Etc/UTC", "GMT", "Etc/GMT", "Z"];

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
    /// The stream's last `rate_limit_event`, when it rejected a request.
    rejection: Option<RateLimitRejection>,
}

/// A `rate_limit_event` whose request the login's usage limits rejected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RateLimitRejection {
    /// `resetsAt`, in Unix epoch seconds; `None` when it is missing or no time.
    resets_at: Option<OffsetDateTime>,
    /// `errorCode` `credits_required`: the subscription's included usage is spent, and only
    /// usage credits would go on.
    credits_required: bool,
}

impl RateLimitRejection {
    /// The rejection a `rate_limit_event` reports; `None` for an event that allowed its request.
    fn of(event: &Value) -> Option<Self> {
        let info = event.get("rate_limit_info")?;
        if info.get("status").and_then(Value::as_str) != Some("rejected") {
            return None;
        }
        Some(Self {
            resets_at: info
                .get("resetsAt")
                .and_then(Value::as_i64)
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok()),
            credits_required: info.get("errorCode").and_then(Value::as_str)
                == Some("credits_required"),
        })
    }
}

impl ResultEnvelope {
    /// The envelope of a process's stream on its stdout: its last `result` line, with the last
    /// `rate_limit_event` before it. `None` when no line is a result.
    pub fn parse(stdout: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(stdout);
        let mut result = None;
        let mut rate_limit_event = None;
        for message in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        {
            match message.get("type").and_then(Value::as_str) {
                Some("result") => result = Some(message),
                Some("rate_limit_event") => rate_limit_event = Some(message),
                _ => {}
            }
        }
        let mut envelope = Self::of(&result?);
        envelope.rejection = rate_limit_event.as_ref().and_then(RateLimitRejection::of);
        Some(envelope)
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
            rejection: None,
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
    /// `--max-budget-usd`. The runner does not set it; the CLI's answer stays readable.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimit {
    pub kind: UsageLimitKind,
    /// The reset time the CLI reported, `None` when it named none the runner can read.
    pub resets_at: Option<OffsetDateTime>,
    /// The line of the CLI's limit message, shortened, for the log when the reset time cannot
    /// be read; `None` when the CLI reported the limit without one.
    pub message: Option<String>,
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

    /// The usage limit an error envelope reports. The CLI's limit message (`You've hit your
    /// weekly limit · resets Mon 12:00am`) or its typed `usage_limit_reached` says that it is
    /// one, and so does a rejecting `rate_limit_event` before a 429. The event's `resetsAt` is
    /// the reset time; the message's is read only without it.
    fn usage_limit(&self, now: OffsetDateTime) -> Option<UsageLimit> {
        if !self.is_error {
            return None;
        }
        let message = self.texts.iter().find_map(|text| LimitMessage::find(text));
        let typed = self.api_error.as_deref() == Some("usage_limit_reached");
        let rejected = self.rejection.is_some() && self.api_error_status == Some(429);
        if message.is_none() && !typed && !rejected {
            return None;
        }
        let rejection = self.rejection.unwrap_or_default();
        let kind = match &message {
            Some(message) => message.kind,
            None if rejection.credits_required => UsageLimitKind::Spend,
            None => UsageLimitKind::Session,
        };
        Some(UsageLimit {
            kind,
            resets_at: rejection.resets_at.or_else(|| {
                message
                    .as_ref()
                    .and_then(|message| reset_time(message.text, now))
            }),
            message: message.map(|message| message.line()),
        })
    }
}

/// A CLI limit message in an envelope text, such as `You've hit your weekly limit · resets
/// Mon 12:00am`: the kind of limit it names and the text from where it starts.
struct LimitMessage<'a> {
    kind: UsageLimitKind,
    text: &'a str,
}

impl<'a> LimitMessage<'a> {
    fn find(text: &'a str) -> Option<Self> {
        static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?i)\b(?:(?P<credits>you['’]re out of usage credits)|(?:you['’]ve|you have) (?:hit|reached) your (?P<limit>[^·.\n]+?)(?:\s*[·.]|\s*$))",
            )
            .unwrap_or_else(|error| panic!("limit message pattern: {error}"))
        });
        let captures = LIMIT.captures(text)?;
        let start = captures.get(0)?.start();
        if captures.name("credits").is_some() {
            return Some(Self {
                kind: UsageLimitKind::Spend,
                text: &text[start..],
            });
        }
        let limit = captures.name("limit")?.as_str();
        Some(Self {
            kind: limit_kind(&limit.to_ascii_lowercase()),
            text: &text[start..],
        })
    }

    /// The message's line, shortened, as the log shows it.
    fn line(&self) -> String {
        self.text
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(MAX_LIMIT_MESSAGE_CHARS)
            .collect()
    }
}

/// The kind of limit a limit message names, lowercase: `session limit`, `weekly limit`, `opus
/// limit` or a spend limit such as `org's monthly spend limit`.
fn limit_kind(limit: &str) -> UsageLimitKind {
    if limit.contains("5-hour") || limit.contains("session") {
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
    }
}

/// The reset time of a limit message, as the CLI writes it in the process's time zone, which the
/// runner sets to UTC: `resets 3:45pm`, `resets Mon 12:00am`, `resets Nov 3, 9am` or `resets Jan
/// 3, 2027, 9am`, each optionally followed by its zone, such as `(UTC)`. A time without a date is
/// the next such time after `now`, and one with a weekday the next such day; a time that names
/// another zone than UTC is not read.
fn reset_time(text: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    static RESET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"resets (?:(?P<weekday>Mon|Tue|Wed|Thu|Fri|Sat|Sun) |(?P<month>[A-Z][a-z]{2}) (?P<day>\d{1,2}), (?:(?P<year>\d{4}), )?)?(?P<hour>\d{1,2})(?::(?P<minute>\d{2}))?\s?(?P<meridiem>[ap]m)(?: \((?P<zone>[^)]+)\))?",
        )
        .unwrap_or_else(|error| panic!("reset time pattern: {error}"))
    });
    let reset = RESET.captures(text)?;
    if reset
        .name("zone")
        .is_some_and(|zone| !UTC_ZONES.contains(&zone.as_str()))
    {
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
    if let Some(weekday) = reset.name("weekday") {
        let weekday = weekday_of(weekday.as_str())?;
        return (0..=7)
            .map(|days| {
                PrimitiveDateTime::new(now.date(), time).assume_utc() + Duration::days(days)
            })
            .find(|at| at.weekday() == weekday && *at > now);
    }
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

fn weekday_of(name: &str) -> Option<Weekday> {
    const WEEKDAYS: [(&str, Weekday); 7] = [
        ("Mon", Weekday::Monday),
        ("Tue", Weekday::Tuesday),
        ("Wed", Weekday::Wednesday),
        ("Thu", Weekday::Thursday),
        ("Fri", Weekday::Friday),
        ("Sat", Weekday::Saturday),
        ("Sun", Weekday::Sunday),
    ];
    WEEKDAYS
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, weekday)| *weekday)
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

    /// The envelope of a stream whose only line is `value` as a `result` message.
    fn envelope(mut value: Value) -> ResultEnvelope {
        if let Some(message) = value.as_object_mut() {
            message.insert("type".to_owned(), json!("result"));
        }
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
        assert_eq!(
            ResultEnvelope::parse(
                b"{\"type\":\"system\",\"subtype\":\"init\"}\n{\"subtype\":\"success\",\"total_cost_usd\":1.0}\n"
            ),
            None,
            "only a line of type result is the envelope"
        );
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

    /// What an error envelope with `result` reports, read at `NOW`: the limit's kind and reset
    /// time, or `None` when it is no usage limit.
    fn limit_in(result: &str) -> Option<(UsageLimitKind, Option<OffsetDateTime>)> {
        match envelope(json!({
            "subtype": "success",
            "is_error": true,
            "api_error_status": 429,
            "result": result,
        }))
        .outcome(NOW)
        {
            EnvelopeOutcome::UsageLimit(limit) => Some((limit.kind, limit.resets_at)),
            _ => None,
        }
    }

    /// The limit messages of the CLI's error reference (code.claude.com/docs/en/errors, "Usage
    /// limits"), read without a `rate_limit_event`, as from a CLI that sends none. The CLI writes
    /// times in the process's zone, which the runner sets to UTC, and names the zone only in some
    /// versions. `NOW` is a Wednesday.
    #[test]
    fn should_read_the_kind_and_reset_time_of_the_cli_s_limit_messages() {
        let monday = datetime!(2026-11-02 00:00 UTC);
        for (message, expected) in [
            (
                "You've hit your session limit · resets 3:45pm",
                (
                    UsageLimitKind::Session,
                    Some(datetime!(2026-10-28 15:45 UTC)),
                ),
            ),
            (
                "You've hit your weekly limit · resets Mon 12:00am",
                (UsageLimitKind::Weekly, Some(monday)),
            ),
            (
                "You've hit your Opus limit · resets 3:45pm",
                (UsageLimitKind::Model, Some(datetime!(2026-10-28 15:45 UTC))),
            ),
            (
                "You've hit your Sonnet limit · resets 1:15pm",
                (UsageLimitKind::Model, Some(datetime!(2026-10-29 13:15 UTC))),
            ),
            (
                "You've hit your monthly spend limit · raise it at https://claude.ai/settings/usage?from=cc_cli_limit_message",
                (UsageLimitKind::Spend, None),
            ),
            (
                "You've hit your individual spend limit · ask your admin for a higher limit · your session limit resets 3:45pm",
                (UsageLimitKind::Spend, Some(datetime!(2026-10-28 15:45 UTC))),
            ),
            (
                "You've hit your org's monthly spend limit · visit https://claude.ai/admin-settings/usage to raise it",
                (UsageLimitKind::Spend, None),
            ),
            (
                "You've hit your team's shared budget · ask your admin to raise it at https://claude.ai/admin-settings/usage",
                (UsageLimitKind::Spend, None),
            ),
            (
                "You've hit your individual usage limit",
                (UsageLimitKind::Spend, None),
            ),
            (
                "You've hit your 5-hour limit · resets 3pm (UTC)",
                (
                    UsageLimitKind::Session,
                    Some(datetime!(2026-10-28 15:00 UTC)),
                ),
            ),
            (
                "You've hit your weekly limit · resets Nov 3, 9am (UTC) · progress saved",
                (
                    UsageLimitKind::Weekly,
                    Some(datetime!(2026-11-03 09:00 UTC)),
                ),
            ),
            (
                "You’ve hit your Opus limit · resets Jan 2, 2027, 12am (UTC)",
                (UsageLimitKind::Model, Some(datetime!(2027-01-02 00:00 UTC))),
            ),
            (
                "API Error: You've hit your Sonnet limit · resets 11:45pm (UTC)",
                (UsageLimitKind::Model, Some(datetime!(2026-10-28 23:45 UTC))),
            ),
            (
                "You've hit your session limit · resets Wed 1pm",
                (
                    UsageLimitKind::Session,
                    Some(datetime!(2026-11-04 13:00 UTC)),
                ),
            ),
            (
                "You've hit your 5-hour limit · resets 3pm (Europe/Berlin)",
                (UsageLimitKind::Session, None),
            ),
            (
                "API Error: You're out of usage credits",
                (UsageLimitKind::Spend, None),
            ),
        ] {
            assert_eq!(limit_in(message), Some(expected), "{message}");
        }
        for not_a_limit in [
            "API Error: Server is temporarily limiting requests (not your usage limit)",
            "API Error: Request rejected (429) · this may be a temporary capacity issue. If it persists, check https://status.claude.com.",
            "You've used 85% of your session limit · resets 3:45pm",
        ] {
            assert_eq!(limit_in(not_a_limit), None, "{not_a_limit}");
        }
        assert_eq!(
            envelope(
                json!({ "is_error": true, "api_error": "usage_limit_reached", "errors": ["limit"] })
            )
            .outcome(NOW),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: None,
                message: None,
            })
        );
    }

    /// The `rate_limit_event` before the result is the CLI's structured account of the limit: its
    /// `resetsAt` is the reset time whatever the message says, and it marks a 429 whose message
    /// the runner does not recognise as a usage limit.
    #[test]
    fn should_take_the_reset_time_from_the_stream_s_rejecting_rate_limit_event() {
        let resets_at = datetime!(2026-10-30 09:00 UTC);
        let rate_limit_event = |status: &str, extra: Value| {
            let mut info = json!({ "status": status, "resetsAt": resets_at.unix_timestamp() });
            if let (Some(info), Some(extra)) = (info.as_object_mut(), extra.as_object()) {
                info.extend(extra.clone());
            }
            json!({ "type": "rate_limit_event", "rate_limit_info": info, "uuid": "u", "session_id": "s" })
        };
        let stream = |events: &[Value], result: &str, status: u16| {
            let mut lines: Vec<String> = vec![
                json!({ "type": "system", "subtype": "init", "model": "claude-opus-5-5" }).to_string(),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "page text" }] } }).to_string(),
            ];
            lines.extend(events.iter().map(Value::to_string));
            lines.push(
                json!({
                    "type": "result", "subtype": "success", "is_error": true,
                    "api_error_status": status, "result": result, "total_cost_usd": 0.4,
                })
                .to_string(),
            );
            ResultEnvelope::parse(lines.join("\n").as_bytes())
                .unwrap_or_else(|| panic!("no envelope"))
                .outcome(NOW)
        };
        let rejected = rate_limit_event("rejected", json!({}));
        let warned = rate_limit_event("allowed_warning", json!({ "utilization": 0.9 }));

        assert_eq!(
            stream(
                &[warned.clone(), rejected.clone()],
                "You've hit your weekly limit · resets Mon 12:00am",
                429
            ),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Weekly,
                resets_at: Some(resets_at),
                message: Some("You've hit your weekly limit · resets Mon 12:00am".to_owned()),
            })
        );
        assert_eq!(
            stream(
                std::slice::from_ref(&rejected),
                "API Error: 429 rate_limit_error",
                429
            ),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Session,
                resets_at: Some(resets_at),
                message: None,
            })
        );
        assert_eq!(
            stream(
                &[rate_limit_event(
                    "rejected",
                    json!({ "errorCode": "credits_required", "resetsAt": null })
                )],
                "API Error: 429",
                429
            ),
            EnvelopeOutcome::UsageLimit(UsageLimit {
                kind: UsageLimitKind::Spend,
                resets_at: None,
                message: None,
            })
        );
        assert_eq!(
            stream(
                &[rejected.clone(), warned],
                "API Error: 429 rate_limit_error",
                429
            ),
            EnvelopeOutcome::Failed,
            "a later event that allowed a request ends the rejection"
        );
        assert_eq!(
            stream(&[rejected], "API Error: 500 internal error", 500),
            EnvelopeOutcome::Failed
        );
    }

    /// A message whose reset time the runner cannot read keeps its own line for the log, and
    /// nothing the agent read after it.
    #[test]
    fn should_keep_only_the_line_of_a_limit_message_whose_reset_cannot_be_read() {
        let read = limit_message_of(
            "API Error: You've hit your weekly limit · resets in a while\nproduct page text the agent read",
        );

        assert_eq!(
            read,
            Some(UsageLimit {
                kind: UsageLimitKind::Weekly,
                resets_at: None,
                message: Some("You've hit your weekly limit · resets in a while".to_owned()),
            })
        );
    }

    fn limit_message_of(result: &str) -> Option<UsageLimit> {
        match envelope(json!({ "is_error": true, "api_error_status": 429, "result": result }))
            .outcome(NOW)
        {
            EnvelopeOutcome::UsageLimit(limit) => Some(limit),
            _ => None,
        }
    }

    #[test]
    fn should_pause_until_the_reset_or_briefly_when_it_cannot_be_read() {
        let read = |resets_at| UsageLimit {
            kind: UsageLimitKind::Weekly,
            resets_at,
            message: None,
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
