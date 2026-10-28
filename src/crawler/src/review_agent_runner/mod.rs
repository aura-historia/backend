//! `review-agent-runner`: works the review console's pending reviews with the two review agents of
//! `agent` authoring mode, headless, through the Claude Code CLI that is logged in on the machine.
//! Per review it starts a fresh author `claude -p` process and then a fresh reviewer process, each
//! with only its role's `review-mcp` tools, and it records each process's report, tokens and cost.
//! It is an operator of the console like a human with a browser: it lists reviews and events with
//! the author agent's token, holds no database access and no review rule of its own, and the
//! console's role refusals and approval guard apply to everything the agents do. Everything
//! specific to Claude Code lives here, outside the crawler's services.
//!
//! One run is one pass over the pending reviews, under a lock so one runner works at a time; a
//! systemd timer starts the runs in production, `--once` starts one for development. The contract
//! is in `docs/crawler/operations.md` (review agent runner).

mod digest;
mod envelope;
mod invocation;
mod ledger;
mod login;
mod pass;
mod process;
mod queue;
mod reports;
mod settings;
mod state;

pub use pass::{PassStop, PassSummary};
pub use settings::{
    AgentProcessSettings, RunMode, RunnerLocation, RunnerSettings, RunnerSettingsError,
    current_location,
};

use digest::{BudgetState, Digest};
use ledger::{FIVE_HOURS, LEDGER_RETENTION, ONE_WEEK, ProcessLedger};
use login::{LoginError, probe_login};
use pass::{PassError, usage_limit_name, work_pending_reviews};
use queue::review_events_since;
use state::{RunnerLock, RunnerState, StateError, load_state, save_state};

use crate::review_mcp::{AgentRole, ConsoleClient, ConsoleClientError, RoleTokenFile};
use std::path::PathBuf;
use time::{Duration, OffsetDateTime};
use tracing::{info, warn};

/// The name `value` is serialized under, such as `LIMIT_REACHED`, for a log field or the digest;
/// empty for none.
fn wire_name<T: serde::Serialize>(value: Option<T>) -> String {
    value
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// How often the timer's runs write a digest.
const DIGEST_INTERVAL: Duration = Duration::days(1);

/// What one start of the runner did.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// Another runner held the lock, so this start did nothing.
    Locked,
    Ran {
        summary: PassSummary,
        /// The digest this run wrote, if one was due.
        digest: Option<PathBuf>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("cannot create the runner's state directory {}", path.display())]
    StateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Ledger(#[from] ledger::LedgerError),
    #[error(
        "the runner refuses to start: the review agents' token file cannot be read, so the crawler is not in CRAWLER_AUTHORING_MODE=agent or CRAWLER_REVIEW_AGENT_TOKEN_FILE names another file"
    )]
    NotAgentMode(#[source] ConsoleClientError),
    #[error("the runner refuses to start: {0}")]
    Login(#[from] LoginError),
    #[error("the review agents' pass failed")]
    Pass(#[from] PassError),
}

/// One run: takes the lock, checks that the crawler is in `agent` mode and that the CLI is
/// logged in with a Claude subscription, works the pending reviews, and writes the digest when it
/// is due (a timer run) or right away (`--once`).
pub async fn run(settings: &RunnerSettings) -> Result<RunOutcome, RunnerError> {
    std::fs::create_dir_all(&settings.state_dir).map_err(|source| RunnerError::StateDirectory {
        path: settings.state_dir.clone(),
        source,
    })?;
    let Some(_lock) = RunnerLock::acquire(&settings.state_dir)? else {
        info!(
            state_dir = %settings.state_dir.display(),
            "Another review agent runner holds the lock; this start does nothing"
        );
        return Ok(RunOutcome::Locked);
    };
    let console = ConsoleClient::new(
        &settings.console.console_url,
        RoleTokenFile::new(settings.console.token_file.clone(), AgentRole::Author),
    )
    .map_err(RunnerError::NotAgentMode)?;
    let login = probe_login(&settings.claude_bin).await?;
    info!(
        mode = ?settings.mode,
        auth_method = %login.method,
        organization = login.organization.as_deref().unwrap_or(""),
        subscription = login.subscription.as_deref().unwrap_or(""),
        console_url = %settings.console.console_url,
        state_dir = %settings.state_dir.display(),
        "Review agent runner started with the CLI's Claude login"
    );
    let started = OffsetDateTime::now_utc();
    let mut state = load_state(&settings.state_dir)?;
    state.expire(started);
    let mut ledger = ProcessLedger::open(&settings.state_dir)?;
    ledger.prune(started - LEDGER_RETENTION)?;

    let (pass, mut state, ledger) = work_pending_reviews(settings, &console, state, ledger).await;
    save_state(&settings.state_dir, &state)?;
    let summary = pass?;
    let now = OffsetDateTime::now_utc();
    info!(
        pending = summary.pending,
        cooling_down = summary.cooling_down,
        worked = summary.worked,
        processes = summary.processes,
        decided = summary.decided,
        stopped = %PassStop::describe(summary.stopped),
        five_hour_cost_usd = ledger.cost_within(FIVE_HOURS, now),
        weekly_cost_usd = ledger.cost_within(ONE_WEEK, now),
        "Review agents pass finished"
    );
    let digest = write_digest(settings, &console, &mut state, &ledger, now).await;
    if digest.is_some() {
        save_state(&settings.state_dir, &state)?;
    }
    Ok(RunOutcome::Ran { summary, digest })
}

/// Writes the digest a run owes: a timer run once a day has passed since the last one, a `--once`
/// run of the last day right away. A digest that cannot be written is logged and owed again.
async fn write_digest(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    state: &mut RunnerState,
    ledger: &ProcessLedger,
    now: OffsetDateTime,
) -> Option<PathBuf> {
    let from = match settings.mode {
        RunMode::Once => now - DIGEST_INTERVAL,
        RunMode::Scheduled => match state.last_digest {
            Some(last) if now - last < DIGEST_INTERVAL => return None,
            Some(last) => last.max(now - LEDGER_RETENTION),
            None => now - DIGEST_INTERVAL,
        },
    };
    let events = match review_events_since(console, from).await {
        Ok(events) => events,
        Err(error) => {
            warn!(
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Cannot read the review events for the digest; the next run writes it"
            );
            return None;
        }
    };
    let budget = BudgetState {
        five_hour_cost_usd: ledger.cost_within(FIVE_HOURS, now),
        five_hour_budget_usd: settings.five_hour_budget_usd,
        weekly_cost_usd: ledger.cost_within(ONE_WEEK, now),
        weekly_budget_usd: settings.weekly_budget_usd,
        paused_until: state
            .pause_at(now)
            .map(|pause| (pause.until, usage_limit_name(pause.limit).to_owned())),
    };
    let digest = Digest::of(
        from,
        now,
        events,
        ledger.since(from).cloned().collect(),
        budget,
    );
    match digest.write(&settings.state_dir) {
        Ok(path) => {
            info!(
                path = %path.display(),
                from = %digest::timestamp(digest.from),
                to = %digest::timestamp(digest.to),
                approved_by_agents = digest.approved.agents,
                approved_by_others = digest.approved.others,
                rejected_by_agents = digest.rejected.agents,
                rejected_by_others = digest.rejected.others,
                needs_repair_by_agents = digest.needs_repair.agents,
                needs_repair_by_others = digest.needs_repair.others,
                restored = digest.restored,
                refusals_by_failed_check = ?digest.refusals_by_check,
                refusals_by_kind = ?digest.refusals_by_kind,
                processes = ?digest.processes_by_end,
                input_tokens = digest.usage.input_tokens,
                output_tokens = digest.usage.output_tokens,
                cache_read_input_tokens = digest.usage.cache_read_input_tokens,
                cache_creation_input_tokens = digest.usage.cache_creation_input_tokens,
                total_cost_usd = digest.cost_usd,
                reviews = digest.reviews.len(),
                "Review agents digest"
            );
            if settings.mode == RunMode::Scheduled {
                state.last_digest = Some(now);
            }
            Some(path)
        }
        Err(error) => {
            warn!(error = %error, "Cannot write the review agents digest; the next run writes it");
            None
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::CrawlerReviewId;
    use crate::review::model::CrawlerReview;
    use crate::review_agent_runner::ledger::LEDGER_FILE;
    use crate::review_agent_runner::ledger::tests::StateDirectory;
    use crate::review_agent_runner::settings::RunnerLocation;
    use listing_source_core::ListingSourceId;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const AUTHOR_TOKEN: &str = "authortokenfortheconsole";
    const REVIEWER_TOKEN: &str = "reviewertokenfortheconsole";

    /// A review console that answers the review queue and the event list to the author's token.
    struct FakeConsole {
        url: String,
    }

    impl FakeConsole {
        async fn start(reviews: Vec<CrawlerReview>, events: Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("bind: {error}"));
            let url = format!(
                "http://{}",
                listener
                    .local_addr()
                    .unwrap_or_else(|error| panic!("{error}"))
            );
            let reviews = Arc::new(serde_json::to_string(&reviews).unwrap_or_default());
            let events = Arc::new(events.to_string());
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request).to_string();
                    let authorized = request
                        .lines()
                        .any(|line| line == format!("authorization: Bearer {AUTHOR_TOKEN}"));
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let (status, body) = if !authorized {
                        (401, "{\"error\":\"unauthorized\"}".to_owned())
                    } else if path == "/api/reviews" {
                        (200, reviews.to_string())
                    } else if path.starts_with("/api/review-events?since=") {
                        (200, events.to_string())
                    } else {
                        (404, "{\"error\":\"not found\"}".to_owned())
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            Self { url }
        }
    }

    fn review(status: &str, crawl_enabled: bool) -> CrawlerReview {
        let now = OffsetDateTime::now_utc();
        CrawlerReview {
            review_id: CrawlerReviewId::new(),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: Some("Antiques".to_owned()),
            listing_source_crawl_enabled: crawl_enabled,
            domain_id: None,
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            status: status.to_owned(),
            reason: "manual_schema_required".to_owned(),
            candidate_payload: json!({}),
            candidate_version: 1,
            validation_summary: json!({}),
            reviewer_notes: None,
            repair_request: None,
            created: now,
            updated: now,
            reviewed: None,
        }
    }

    /// A `claude` executable that records each call's arguments, input and environment and
    /// answers the canned envelope of its role, or of its review and role.
    struct FakeClaude {
        directory: StateDirectory,
    }

    const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = auth ]; then cat "$dir/auth.json"; exit 0; fi
role=reviewer
case "$*" in *'"--role","author"'*) role=author;; esac
prompt="$(cat)"
review="$(printf '%s' "$prompt" | grep -o 'cr_[0-9a-z]*' | head -n 1)"
call="$dir/calls/$(date +%s%N)-$$-$role"
mkdir -p "$call"
printf '%s\n' "$@" > "$call/argv"
printf '%s' "$prompt" > "$call/stdin"
env > "$call/env"
echo "start $role $review" >> "$dir/order.log"
sleep 0.2
envelope="$dir/envelopes/$review-$role.json"
[ -f "$envelope" ] || envelope="$dir/envelopes/$role.json"
sed "s/__REVIEW_ID__/$review/g" "$envelope"
echo "end $role $review" >> "$dir/order.log"
"#;

    impl FakeClaude {
        fn new() -> Self {
            let directory = StateDirectory::new();
            let path = directory.path().join("claude");
            std::fs::write(&path, FAKE_CLAUDE).unwrap_or_default();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("{error}"));
            std::fs::create_dir(directory.path().join("envelopes")).unwrap_or_default();
            let fake = Self { directory };
            fake.login(json!({ "loggedIn": true, "authMethod": "claude.ai", "orgName": "Aura" }));
            fake.answer("author", author_envelope("STAGED", 0.5));
            fake.answer("reviewer", reviewer_envelope("APPROVE", "ACCEPTED", 0.25));
            fake
        }

        fn path(&self) -> PathBuf {
            self.directory.path().join("claude")
        }

        fn login(&self, status: Value) {
            std::fs::write(self.directory.path().join("auth.json"), status.to_string())
                .unwrap_or_default();
        }

        /// Answers `envelope` to the role, or to `<review id>-<role>`.
        fn answer(&self, name: &str, envelope: Value) {
            std::fs::write(
                self.directory.path().join(format!("envelopes/{name}.json")),
                envelope.to_string(),
            )
            .unwrap_or_default();
        }

        fn order(&self) -> Vec<String> {
            std::fs::read_to_string(self.directory.path().join("order.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// Each call's recorded files, oldest first.
        fn calls(&self) -> Vec<HashMap<&'static str, String>> {
            let mut calls: Vec<PathBuf> = std::fs::read_dir(self.directory.path().join("calls"))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .collect()
                })
                .unwrap_or_default();
            calls.sort();
            calls
                .iter()
                .map(|call| {
                    ["argv", "stdin", "env"]
                        .into_iter()
                        .map(|file| {
                            (
                                file,
                                std::fs::read_to_string(call.join(file)).unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .collect()
        }
    }

    fn author_envelope(action: &str, cost: f64) -> Value {
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 9,
            "duration_ms": 1200, "total_cost_usd": cost,
            "usage": { "input_tokens": 100, "output_tokens": 50 },
            "modelUsage": { "claude-opus-5-5": {} },
            "result": "page text the runner must not log",
            "structured_output": {
                "review_id": "__REVIEW_ID__", "action": action, "candidate_version": 2,
                "summary": "Staged and tested on /item/1.", "risks": [],
            },
        })
    }

    fn reviewer_envelope(decision: &str, outcome: &str, cost: f64) -> Value {
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 5,
            "duration_ms": 800, "total_cost_usd": cost,
            "usage": { "input_tokens": 80, "output_tokens": 20 },
            "structured_output": {
                "review_id": "__REVIEW_ID__", "decision": decision, "confidence": "HIGH",
                "summary": "Checked /item/1.", "risks": [], "field_findings": [],
                "server_answer": {
                    "outcome": outcome,
                    "kind": if outcome == "REFUSED" { "CONFLICT" } else { "" },
                    "failed_checks": if outcome == "REFUSED" { json!(["LIVE_MATRIX_MISSING"]) } else { json!([]) },
                    "detail": "",
                },
            },
        })
    }

    struct Fixture {
        console: FakeConsole,
        claude: FakeClaude,
        state: StateDirectory,
        tokens: StateDirectory,
    }

    impl Fixture {
        async fn new(reviews: Vec<CrawlerReview>) -> Self {
            let events = json!({
                "events": [{
                    "event_id": "cre_x", "review_id": CrawlerReviewId::new().to_string(),
                    "artifact_type": "URL_PATTERN", "listing_source_id": "ls_x",
                    "listing_source_name": "Antiques", "actor": "HUMAN", "action": "RESTORED",
                    "candidate_version": 4, "notes": "pattern took categories",
                    "created": (OffsetDateTime::now_utc() - Duration::hours(1))
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                }],
                "truncated": false,
            });
            let tokens = StateDirectory::new();
            std::fs::write(
                tokens.path().join("review-agent-tokens.json"),
                json!({ "author": AUTHOR_TOKEN, "reviewer": REVIEWER_TOKEN }).to_string(),
            )
            .unwrap_or_default();
            Self {
                console: FakeConsole::start(reviews, events).await,
                claude: FakeClaude::new(),
                state: StateDirectory::new(),
                tokens,
            }
        }

        fn settings(&self, mode_args: &[&str], env: &[(&str, &str)]) -> RunnerSettings {
            let claude = self.claude.path();
            let state = self.state.path().display().to_string();
            let token_file = self.tokens.path().join("review-agent-tokens.json");
            let mut values: HashMap<String, String> = HashMap::from([
                (
                    "CRAWLER_REVIEW_CONSOLE_URL".to_owned(),
                    self.console.url.clone(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_TOKEN_FILE".to_owned(),
                    token_file.display().to_string(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN".to_owned(),
                    claude.display().to_string(),
                ),
                ("CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR".to_owned(), state),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES".to_owned(),
                    "1".to_owned(),
                ),
            ]);
            values.extend(
                env.iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
            RunnerSettings::from_args_and_env(
                mode_args.iter().map(|arg| (*arg).to_owned()),
                |name| values.get(name).cloned(),
                &RunnerLocation {
                    working_dir: self.state.path().to_path_buf(),
                    executable: PathBuf::from("/opt/crawler/bin/review-agent-runner"),
                },
            )
            .unwrap_or_else(|error| panic!("{error}"))
        }

        fn ledger(&self) -> Vec<Value> {
            std::fs::read_to_string(self.state.path().join(LEDGER_FILE))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
    }

    async fn run_with(settings: &RunnerSettings) -> RunOutcome {
        run(settings)
            .await
            .unwrap_or_else(|error| panic!("{error}: {error:?}"))
    }

    fn summary(outcome: &RunOutcome) -> PassSummary {
        match outcome {
            RunOutcome::Ran { summary, .. } => summary.clone(),
            RunOutcome::Locked => panic!("the run found the lock held"),
        }
    }

    #[tokio::test]
    async fn should_work_each_pending_review_with_a_fresh_author_then_reviewer_process() {
        let first = review("PENDING_REVIEW", true);
        let second = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![
            first.clone(),
            review("PENDING_REVIEW", false),
            review("APPROVED", true),
            second.clone(),
        ])
        .await;

        let outcome = run_with(&fixture.settings(&["--once"], &[])).await;

        let summary = summary(&outcome);
        assert_eq!(
            (
                summary.pending,
                summary.worked,
                summary.processes,
                summary.decided
            ),
            (2, 2, 4, 2)
        );
        let (first_id, second_id) = (first.review_id, second.review_id);
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {first_id}"),
                format!("end author {first_id}"),
                format!("start reviewer {first_id}"),
                format!("end reviewer {first_id}"),
                format!("start author {second_id}"),
                format!("end author {second_id}"),
                format!("start reviewer {second_id}"),
                format!("end reviewer {second_id}"),
            ]
        );
        let calls = fixture.claude.calls();
        assert_eq!(calls.len(), 4);
        for call in &calls {
            let argv: Vec<&str> = call["argv"].lines().collect();
            for flag in ["-p", "--no-session-persistence", "--strict-mcp-config"] {
                assert!(argv.contains(&flag), "{flag}: {argv:?}");
            }
            assert!(!argv.contains(&"--bare"));
            for token in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                for (file, text) in call {
                    assert!(!text.contains(token), "a token in the child's {file}");
                }
            }
            assert!(
                call["env"]
                    .lines()
                    .any(|line| line == "DISABLE_AUTOUPDATER=1")
            );
            assert!(!call["stdin"].is_empty());
        }
        let ledger = fixture.ledger();
        assert_eq!(ledger.len(), 4);
        assert_eq!(ledger[0]["role"], "author");
        assert_eq!(ledger[0]["report"]["action"], "STAGED");
        assert_eq!(ledger[1]["report"]["decision"], "APPROVE");
        assert_eq!(ledger[1]["total_cost_usd"], 0.25);
        assert!(
            !std::fs::read_to_string(fixture.state.path().join(LEDGER_FILE))
                .unwrap_or_default()
                .contains("page text the runner must not log")
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
        let RunOutcome::Ran {
            digest: Some(digest),
            ..
        } = outcome
        else {
            panic!("--once wrote no digest");
        };
        let digest = std::fs::read_to_string(digest).unwrap_or_default();
        assert!(digest.contains("| Restored | – | 1 |"), "{digest}");
        assert!(digest.contains(&first_id.to_string()), "{digest}");
        assert!(digest.contains("4 processes (4 reported)"), "{digest}");
    }

    #[tokio::test]
    async fn should_cool_down_a_review_whose_author_ends_without_a_decision() {
        let limited = review("PENDING_REVIEW", true);
        let other = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![limited.clone(), other.clone()]).await;
        fixture.claude.answer(
            &format!("{}-author", limited.review_id),
            json!({
                "type": "result", "subtype": "error_max_turns", "is_error": true,
                "terminal_reason": "max_turns", "total_cost_usd": 0.75, "num_turns": 80,
            }),
        );
        let settings = fixture.settings(&[], &[]);

        let first_run = summary(&run_with(&settings).await);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!((first_run.worked, first_run.processes), (2, 3));
        assert_eq!(first_run.decided, 1);
        let ledger = fixture.ledger();
        assert_eq!(ledger[0]["end"], "LIMIT_REACHED");
        assert_eq!(ledger[0]["limit"], "TURNS");
        assert_eq!(ledger[0]["subtype"], "error_max_turns");
        assert_eq!(ledger[0]["terminal_reason"], "max_turns");
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.contains_key(&limited.review_id));
        assert!(!state.cooldowns.contains_key(&other.review_id));
        assert_eq!(
            (second_run.cooling_down, second_run.worked),
            (1, 1),
            "the second run works only the review that is not cooling down"
        );
        assert!(
            fixture
                .claude
                .order()
                .iter()
                .skip(6)
                .all(|line| line.ends_with(&other.review_id.to_string()))
        );
    }

    #[tokio::test]
    async fn should_pause_on_a_usage_limit_and_start_nothing_until_it_resets() {
        let limited = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![limited.clone(), review("PENDING_REVIEW", true)]).await;
        // The CLI names the year only when the reset falls into another year.
        let reset = OffsetDateTime::now_utc() + Duration::days(2);
        let year = if reset.year() == OffsetDateTime::now_utc().year() {
            String::new()
        } else {
            format!("{}, ", reset.year())
        };
        let month = &format!("{:?}", reset.month())[..3];
        fixture.claude.answer(
            "author",
            json!({
                "type": "result", "subtype": "success", "is_error": true, "api_error_status": 429,
                "result": format!(
                    "You've hit your weekly limit · resets {month} {}, {year}9am (UTC)",
                    reset.day()
                ),
            }),
        );
        let settings = fixture.settings(&[], &[]);

        let first_run = summary(&run_with(&settings).await);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!(first_run.processes, 1);
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        let pause = state
            .paused
            .unwrap_or_else(|| panic!("no pause: {state:?}"));
        assert_eq!(pause.limit, envelope::UsageLimitKind::Weekly);
        assert!(pause.reported_reset);
        assert_eq!((pause.until.date(), pause.until.hour()), (reset.date(), 9));
        assert!(matches!(first_run.stopped, Some(PassStop::Paused(_))));
        assert!(
            state.cooldowns.is_empty(),
            "a paused review does not cool down"
        );
        assert_eq!(second_run.processes, 0);
        assert_eq!(second_run.stopped, Some(PassStop::Paused(pause.until)));
        assert_eq!(fixture.ledger()[0]["usage_limit"], "WEEKLY");
    }

    #[tokio::test]
    async fn should_start_no_process_once_a_soft_budget_is_spent() {
        let fixture = Fixture::new(vec![
            review("PENDING_REVIEW", true),
            review("PENDING_REVIEW", true),
        ])
        .await;
        let settings = fixture.settings(
            &[],
            &[("CRAWLER_REVIEW_AGENT_RUNNER_FIVE_HOUR_BUDGET_USD", "0.6")],
        );

        let run = summary(&run_with(&settings).await);

        assert_eq!((run.worked, run.processes), (1, 2));
        assert_eq!(run.stopped, Some(PassStop::BudgetSpent));
    }

    #[tokio::test]
    async fn should_bound_how_many_processes_run_at_once() {
        let fixture = Fixture::new((0..4).map(|_| review("PENDING_REVIEW", true)).collect()).await;
        let settings = fixture.settings(&[], &[("CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES", "2")]);

        let run = summary(&run_with(&settings).await);

        assert_eq!(run.processes, 8);
        let mut running = 0_i32;
        let mut most = 0;
        for line in fixture.claude.order() {
            running += if line.starts_with("start") { 1 } else { -1 };
            most = most.max(running);
        }
        assert!((1..=2).contains(&most), "{most}");
    }

    #[tokio::test]
    async fn should_refuse_to_run_without_the_token_file_or_a_subscription_login() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(&[], &[]);
        fixture
            .claude
            .login(json!({ "loggedIn": false, "authMethod": "none" }));

        let logged_out = run(&settings).await;
        std::fs::remove_file(&settings.console.token_file).unwrap_or_default();
        let no_token_file = run(&settings).await;

        assert!(matches!(
            logged_out,
            Err(RunnerError::Login(LoginError::NotLoggedIn))
        ));
        assert!(matches!(no_token_file, Err(RunnerError::NotAgentMode(_))));
        assert!(fixture.claude.calls().is_empty());
    }

    #[tokio::test]
    async fn should_do_nothing_while_another_runner_holds_the_lock() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(&[], &[]);
        let _held = RunnerLock::acquire(fixture.state.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("lock taken"));

        let outcome = run_with(&settings).await;

        assert_eq!(outcome, RunOutcome::Locked);
        assert!(fixture.claude.calls().is_empty());
        assert!(!Path::new(&fixture.state.path().join(LEDGER_FILE)).exists());
    }

    #[tokio::test]
    async fn should_write_the_timer_s_digest_once_a_day() {
        let fixture = Fixture::new(vec![]).await;
        let settings = fixture.settings(&[], &[]);

        let first = run_with(&settings).await;
        let second = run_with(&settings).await;

        assert!(matches!(
            first,
            RunOutcome::Ran {
                digest: Some(_),
                ..
            }
        ));
        assert!(matches!(second, RunOutcome::Ran { digest: None, .. }));
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.last_digest.is_some());
    }
}
