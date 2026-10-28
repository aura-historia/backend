//! How `review-agent-runner` is started: whether it runs every interval until stopped or once
//! (`--once`), and from its environment the review console, the CLI and `review-mcp` it starts, its
//! state directory, each agent role's model, effort, turn and budget caps, the run's limits and the
//! interval between runs.
//! An empty or whitespace-only setting counts as unset; any other value that is not accepted fails
//! startup with the variable's name, so a mistyped limit never silently widens what the agents
//! may spend.

use super::invocation::ChildEnvironment;
use crate::review::agent_decisions::{AGENT_MIN_LABELLED_URLS_ENV, DEFAULT_MIN_LABELLED_URLS};
use crate::review_mcp::{AgentRole, ReviewConsoleAccess, ReviewMcpSettingsError};
use large_language_model::ClaudeEffort;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CLAUDE_BIN_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN";
pub const REVIEW_MCP_BIN_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_REVIEW_MCP_BIN";
pub const STATE_DIR_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR";
pub const MAX_REVIEWS_PER_RUN_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_MAX_REVIEWS_PER_RUN";
pub const MAX_PROCESSES_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES";
pub const COOLDOWN_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_COOLDOWN_MINUTES";
pub const PROCESS_TIMEOUT_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_PROCESS_TIMEOUT_MINUTES";
pub const FIVE_HOUR_BUDGET_USD_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_FIVE_HOUR_BUDGET_USD";
pub const WEEKLY_BUDGET_USD_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_WEEKLY_BUDGET_USD";
pub const INTERVAL_SECONDS_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS";

/// The model both agents run on unless their role's setting names another.
const DEFAULT_MODEL: &str = "claude-opus-5-5";
const DEFAULT_EFFORT: ClaudeEffort = ClaudeEffort::High;
const DEFAULT_AUTHOR_MAX_TURNS: NonZeroU32 = NonZeroU32::new(80).unwrap();
const DEFAULT_REVIEWER_MAX_TURNS: NonZeroU32 = NonZeroU32::new(50).unwrap();
const DEFAULT_AUTHOR_MAX_BUDGET_USD: f64 = 5.0;
const DEFAULT_REVIEWER_MAX_BUDGET_USD: f64 = 3.0;
const DEFAULT_MAX_REVIEWS_PER_RUN: NonZeroU32 = NonZeroU32::new(4).unwrap();
const DEFAULT_MAX_PROCESSES: NonZeroU32 = NonZeroU32::new(2).unwrap();
const DEFAULT_COOLDOWN_MINUTES: NonZeroU32 = NonZeroU32::new(180).unwrap();
const DEFAULT_PROCESS_TIMEOUT_MINUTES: NonZeroU32 = NonZeroU32::new(60).unwrap();
const DEFAULT_FIVE_HOUR_BUDGET_USD: f64 = 20.0;
const DEFAULT_WEEKLY_BUDGET_USD: f64 = 100.0;
const DEFAULT_INTERVAL_SECONDS: NonZeroU32 = NonZeroU32::new(300).unwrap();
/// The largest dollar amount a cap or soft budget accepts; anything larger is a typo.
const MAX_USD: f64 = 10_000.0;

/// What one start of the runner does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// The service: a run every interval until it is stopped, each a pass over the pending reviews
    /// and the daily digest once a day has passed since the last one.
    Loop,
    /// A manual run (`--once`): one pass, then a digest of the last day right away.
    Once,
}

/// What one agent process of a role is started with.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProcessSettings {
    /// A model alias or full model name, as `claude --model` takes it.
    pub model: String,
    pub effort: ClaudeEffort,
    pub max_turns: NonZeroU32,
    /// The process's spend cap, `claude --max-budget-usd`.
    pub max_budget_usd: f64,
}

/// The settings of one runner start.
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerSettings {
    pub mode: RunMode,
    /// The console the runner lists reviews and events from, and that `review-mcp` calls; the
    /// token file is absolute, because each agent process starts in an empty directory of its own.
    pub console: ReviewConsoleAccess,
    /// The Claude Code CLI, `claude` on the `PATH` unless set.
    pub claude_bin: PathBuf,
    /// The environment the CLI and the agent processes start with, taken from the runner's.
    pub child_env: ChildEnvironment,
    /// The `review-mcp` binary each agent process starts, by default the one next to the runner.
    pub review_mcp_bin: PathBuf,
    /// Where the runner keeps its lock, state, process ledger and digests.
    pub state_dir: PathBuf,
    pub author: AgentProcessSettings,
    pub reviewer: AgentProcessSettings,
    pub max_reviews_per_run: NonZeroU32,
    /// How many agent processes run at once.
    pub max_processes: NonZeroU32,
    /// How long a review that ended without a decision waits before the next pass.
    pub cooldown: Duration,
    /// How long one agent process may run before it is stopped.
    pub process_timeout: Duration,
    pub five_hour_budget_usd: f64,
    pub weekly_budget_usd: f64,
    /// How long the service rests after a run ends before it starts the next; `--once` ignores it.
    pub interval: Duration,
    /// How many URLs of each label the author agent sets on a URL-pattern review: the console's
    /// `CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS`, which the runner reads to say so in the prompt.
    pub min_labelled_urls: NonZeroU32,
}

impl RunnerSettings {
    pub fn role(&self, role: AgentRole) -> &AgentProcessSettings {
        match role {
            AgentRole::Author => &self.author,
            AgentRole::Reviewer => &self.reviewer,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RunnerSettingsError {
    #[error("usage: review-agent-runner [--once]; {0}")]
    Usage(String),
    #[error(transparent)]
    Console(#[from] ReviewMcpSettingsError),
    #[error("invalid {name}={value:?}: {expected}")]
    Invalid {
        name: String,
        value: String,
        expected: &'static str,
    },
    #[error("cannot resolve {what}: {reason}")]
    Path { what: &'static str, reason: String },
}

/// Where the runner process is, to resolve its defaults: its working directory and its own
/// executable.
pub struct RunnerLocation {
    pub working_dir: PathBuf,
    pub executable: PathBuf,
}

impl RunnerSettings {
    /// Reads the settings from the command-line `args` (without the program name) and the
    /// environment through `env`. `--once` is the only argument.
    pub fn from_args_and_env(
        args: impl IntoIterator<Item = String>,
        env: impl Fn(&str) -> Option<String>,
        location: &RunnerLocation,
    ) -> Result<Self, RunnerSettingsError> {
        let mode = mode_from_args(args)?;
        let setting = |name: &str| {
            env(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let ReviewConsoleAccess {
            console_url,
            token_file,
        } = ReviewConsoleAccess::from_env(&env, || location.working_dir.clone())?;
        let console = ReviewConsoleAccess {
            console_url,
            token_file: absolute(&location.working_dir, token_file),
        };
        let state_dir = setting(STATE_DIR_ENV).map_or_else(
            || location.working_dir.clone(),
            |path| absolute(&location.working_dir, PathBuf::from(path)),
        );
        let review_mcp_bin = match setting(REVIEW_MCP_BIN_ENV) {
            Some(path) => absolute(&location.working_dir, PathBuf::from(path)),
            None => location.executable.with_file_name(review_mcp_file_name()),
        };
        let claude_bin = setting(CLAUDE_BIN_ENV).map_or_else(
            || PathBuf::from("claude"),
            |path| {
                let path = PathBuf::from(path);
                if path.components().count() > 1 {
                    absolute(&location.working_dir, path)
                } else {
                    path
                }
            },
        );
        let child_env = ChildEnvironment::from_env(&env);
        let read = Reader { setting: &setting };
        Ok(Self {
            mode,
            console,
            claude_bin,
            child_env,
            review_mcp_bin,
            state_dir,
            author: read.agent(
                AgentRole::Author,
                DEFAULT_AUTHOR_MAX_TURNS,
                DEFAULT_AUTHOR_MAX_BUDGET_USD,
            )?,
            reviewer: read.agent(
                AgentRole::Reviewer,
                DEFAULT_REVIEWER_MAX_TURNS,
                DEFAULT_REVIEWER_MAX_BUDGET_USD,
            )?,
            max_reviews_per_run: read
                .positive(MAX_REVIEWS_PER_RUN_ENV)?
                .unwrap_or(DEFAULT_MAX_REVIEWS_PER_RUN),
            max_processes: read
                .positive(MAX_PROCESSES_ENV)?
                .unwrap_or(DEFAULT_MAX_PROCESSES),
            cooldown: minutes(
                read.positive(COOLDOWN_MINUTES_ENV)?
                    .unwrap_or(DEFAULT_COOLDOWN_MINUTES),
            ),
            process_timeout: minutes(
                read.positive(PROCESS_TIMEOUT_MINUTES_ENV)?
                    .unwrap_or(DEFAULT_PROCESS_TIMEOUT_MINUTES),
            ),
            five_hour_budget_usd: read
                .usd(FIVE_HOUR_BUDGET_USD_ENV)?
                .unwrap_or(DEFAULT_FIVE_HOUR_BUDGET_USD),
            weekly_budget_usd: read
                .usd(WEEKLY_BUDGET_USD_ENV)?
                .unwrap_or(DEFAULT_WEEKLY_BUDGET_USD),
            interval: Duration::from_secs(u64::from(
                read.positive(INTERVAL_SECONDS_ENV)?
                    .unwrap_or(DEFAULT_INTERVAL_SECONDS)
                    .get(),
            )),
            min_labelled_urls: read
                .positive(AGENT_MIN_LABELLED_URLS_ENV)?
                .unwrap_or(DEFAULT_MIN_LABELLED_URLS),
        })
    }
}

/// The names of a role's settings: `CRAWLER_REVIEW_AGENT_<ROLE>_<SETTING>`.
pub fn role_setting(role: AgentRole, setting: &str) -> String {
    format!(
        "CRAWLER_REVIEW_AGENT_{}_{setting}",
        role.as_str().to_ascii_uppercase()
    )
}

struct Reader<'a, F: Fn(&str) -> Option<String>> {
    setting: &'a F,
}

impl<F: Fn(&str) -> Option<String>> Reader<'_, F> {
    fn agent(
        &self,
        role: AgentRole,
        default_max_turns: NonZeroU32,
        default_max_budget_usd: f64,
    ) -> Result<AgentProcessSettings, RunnerSettingsError> {
        let model_name = role_setting(role, "MODEL");
        let model = match (self.setting)(&model_name) {
            Some(model) if is_model_name(&model) => model,
            Some(model) => {
                return Err(invalid(
                    model_name,
                    model,
                    "a model alias or name of letters, digits, '.', '-', '_', '[' and ']' that starts with a letter or digit",
                ));
            }
            None => DEFAULT_MODEL.to_owned(),
        };
        let effort_name = role_setting(role, "EFFORT");
        let effort = match (self.setting)(&effort_name) {
            Some(effort) => parse_effort(&effort).ok_or_else(|| {
                invalid(effort_name, effort, "one of low, medium, high, xhigh, max")
            })?,
            None => DEFAULT_EFFORT,
        };
        Ok(AgentProcessSettings {
            model,
            effort,
            max_turns: self
                .positive(&role_setting(role, "MAX_TURNS"))?
                .unwrap_or(default_max_turns),
            max_budget_usd: self
                .usd(&role_setting(role, "MAX_BUDGET_USD"))?
                .unwrap_or(default_max_budget_usd),
        })
    }

    fn positive(&self, name: &str) -> Result<Option<NonZeroU32>, RunnerSettingsError> {
        (self.setting)(name)
            .map(|value| {
                value
                    .parse::<NonZeroU32>()
                    .map_err(|_| invalid(name.to_owned(), value, "a whole number of at least 1"))
            })
            .transpose()
    }

    fn usd(&self, name: &str) -> Result<Option<f64>, RunnerSettingsError> {
        (self.setting)(name)
            .map(|value| match value.parse::<f64>() {
                Ok(amount) if amount.is_finite() && amount > 0.0 && amount <= MAX_USD => Ok(amount),
                _ => Err(invalid(
                    name.to_owned(),
                    value,
                    "a dollar amount above 0 and at most 10000, such as 2.50",
                )),
            })
            .transpose()
    }
}

fn invalid(name: String, value: String, expected: &'static str) -> RunnerSettingsError {
    RunnerSettingsError::Invalid {
        name,
        value,
        expected,
    }
}

fn mode_from_args(args: impl IntoIterator<Item = String>) -> Result<RunMode, RunnerSettingsError> {
    let mut mode = RunMode::Loop;
    for arg in args {
        match arg.as_str() {
            "--once" if mode == RunMode::Loop => mode = RunMode::Once,
            "--once" => {
                return Err(RunnerSettingsError::Usage(
                    "--once is given more than once".to_owned(),
                ));
            }
            _ => {
                return Err(RunnerSettingsError::Usage(format!(
                    "unexpected argument {arg:?}"
                )));
            }
        }
    }
    Ok(mode)
}

/// `claude --effort` takes the effort levels of the Claude models.
pub fn parse_effort(value: &str) -> Option<ClaudeEffort> {
    [
        ClaudeEffort::Low,
        ClaudeEffort::Medium,
        ClaudeEffort::High,
        ClaudeEffort::XHigh,
        ClaudeEffort::Max,
    ]
    .into_iter()
    .find(|effort| effort_flag(*effort) == value)
}

/// The value of `claude --effort` for `effort`.
pub const fn effort_flag(effort: ClaudeEffort) -> &'static str {
    match effort {
        ClaudeEffort::Low => "low",
        ClaudeEffort::Medium => "medium",
        ClaudeEffort::High => "high",
        ClaudeEffort::XHigh => "xhigh",
        ClaudeEffort::Max => "max",
    }
}

/// A model name goes into the CLI's argument list as is, so it may hold only the characters model
/// aliases and names use, and starts with a letter or digit, so the CLI never reads it as a flag.
fn is_model_name(value: &str) -> bool {
    value.len() <= 128
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_[]".contains(character))
}

fn minutes(count: NonZeroU32) -> Duration {
    Duration::from_secs(u64::from(count.get()) * 60)
}

fn absolute(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn review_mcp_file_name() -> &'static str {
    if cfg!(windows) {
        "review-mcp.exe"
    } else {
        "review-mcp"
    }
}

/// Where this process runs, for [`RunnerSettings::from_args_and_env`].
pub fn current_location() -> Result<RunnerLocation, RunnerSettingsError> {
    let path_error = |what, error: std::io::Error| RunnerSettingsError::Path {
        what,
        reason: error.to_string(),
    };
    Ok(RunnerLocation {
        working_dir: std::env::current_dir()
            .map_err(|error| path_error("the working directory", error))?,
        executable: std::env::current_exe()
            .map_err(|error| path_error("the runner's own executable", error))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn location() -> RunnerLocation {
        RunnerLocation {
            working_dir: PathBuf::from("/var/lib/crawler-review-agents"),
            executable: PathBuf::from("/opt/crawler/bin/review-agent-runner"),
        }
    }

    fn settings(
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<RunnerSettings, RunnerSettingsError> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        RunnerSettings::from_args_and_env(
            args.iter().map(|arg| (*arg).to_owned()),
            |name| env.get(name).cloned(),
            &location(),
        )
    }

    #[test]
    fn should_run_every_five_minutes_with_the_defaults_when_nothing_is_set() {
        let read = settings(&[], &[]).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(read.mode, RunMode::Loop);
        assert_eq!(read.interval, Duration::from_secs(300));
        assert_eq!(read.console.console_url.as_str(), "http://127.0.0.1:7878/");
        assert_eq!(
            read.console.token_file,
            PathBuf::from("/var/lib/crawler-review-agents/review-agent-tokens.json")
        );
        assert_eq!(read.claude_bin, PathBuf::from("claude"));
        assert_eq!(
            read.review_mcp_bin,
            PathBuf::from("/opt/crawler/bin/review-mcp")
        );
        assert_eq!(
            read.state_dir,
            PathBuf::from("/var/lib/crawler-review-agents")
        );
        assert_eq!(
            read.author,
            AgentProcessSettings {
                model: "claude-opus-5-5".to_owned(),
                effort: ClaudeEffort::High,
                max_turns: NonZeroU32::new(80).unwrap_or(NonZeroU32::MIN),
                max_budget_usd: 5.0,
            }
        );
        assert_eq!(read.reviewer.max_turns.get(), 50);
        assert_eq!(read.reviewer.max_budget_usd, 3.0);
        assert_eq!(read.max_reviews_per_run.get(), 4);
        assert_eq!(read.max_processes.get(), 2);
        assert_eq!(read.cooldown, Duration::from_secs(180 * 60));
        assert_eq!(read.process_timeout, Duration::from_secs(60 * 60));
        assert_eq!(
            (read.five_hour_budget_usd, read.weekly_budget_usd),
            (20.0, 100.0)
        );
        assert_eq!(read.min_labelled_urls.get(), 3);
    }

    #[test]
    fn should_read_each_role_s_caps_and_the_run_s_limits_from_the_environment() {
        let read = settings(
            &["--once"],
            &[
                ("CRAWLER_REVIEW_CONSOLE_URL", "http://127.0.0.1:7979"),
                ("CRAWLER_REVIEW_AGENT_TOKEN_FILE", "tokens.json"),
                (CLAUDE_BIN_ENV, "/usr/local/bin/claude"),
                (REVIEW_MCP_BIN_ENV, "target/debug/review-mcp"),
                (STATE_DIR_ENV, " state "),
                ("CRAWLER_REVIEW_AGENT_AUTHOR_MODEL", "claude-sonnet-5-5"),
                ("CRAWLER_REVIEW_AGENT_AUTHOR_EFFORT", "xhigh"),
                ("CRAWLER_REVIEW_AGENT_REVIEWER_MAX_TURNS", "12"),
                ("CRAWLER_REVIEW_AGENT_REVIEWER_MAX_BUDGET_USD", "0.75"),
                (MAX_REVIEWS_PER_RUN_ENV, "9"),
                (MAX_PROCESSES_ENV, "1"),
                (COOLDOWN_MINUTES_ENV, "5"),
                (PROCESS_TIMEOUT_MINUTES_ENV, "2"),
                (FIVE_HOUR_BUDGET_USD_ENV, "7.5"),
                (WEEKLY_BUDGET_USD_ENV, "40"),
                (INTERVAL_SECONDS_ENV, "30"),
                (AGENT_MIN_LABELLED_URLS_ENV, "4"),
            ],
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(read.mode, RunMode::Once);
        assert_eq!(read.console.console_url.as_str(), "http://127.0.0.1:7979/");
        assert_eq!(
            read.console.token_file,
            PathBuf::from("/var/lib/crawler-review-agents/tokens.json")
        );
        assert_eq!(read.claude_bin, PathBuf::from("/usr/local/bin/claude"));
        assert_eq!(
            read.review_mcp_bin,
            PathBuf::from("/var/lib/crawler-review-agents/target/debug/review-mcp")
        );
        assert_eq!(
            read.state_dir,
            PathBuf::from("/var/lib/crawler-review-agents/state")
        );
        assert_eq!(read.author.model, "claude-sonnet-5-5");
        assert_eq!(read.author.effort, ClaudeEffort::XHigh);
        assert_eq!(read.reviewer.model, "claude-opus-5-5");
        assert_eq!(read.reviewer.max_turns.get(), 12);
        assert_eq!(read.reviewer.max_budget_usd, 0.75);
        assert_eq!(read.max_reviews_per_run.get(), 9);
        assert_eq!(read.max_processes.get(), 1);
        assert_eq!(read.cooldown, Duration::from_secs(300));
        assert_eq!(read.process_timeout, Duration::from_secs(120));
        assert_eq!(
            (read.five_hour_budget_usd, read.weekly_budget_usd),
            (7.5, 40.0)
        );
        assert_eq!(read.interval, Duration::from_secs(30));
        assert_eq!(read.min_labelled_urls.get(), 4);
    }

    #[test]
    fn should_refuse_any_argument_but_once() {
        for args in [&["--once", "--once"][..], &["--loop"][..], &["once"][..]] {
            assert!(
                matches!(settings(args, &[]), Err(RunnerSettingsError::Usage(_))),
                "{args:?}"
            );
        }
    }

    #[test]
    fn should_fail_startup_on_a_value_it_does_not_accept_naming_the_variable() {
        for (name, value) in [
            (
                "CRAWLER_REVIEW_AGENT_AUTHOR_MODEL",
                "opus --dangerously-skip-permissions",
            ),
            (
                "CRAWLER_REVIEW_AGENT_REVIEWER_MODEL",
                "--dangerously-skip-permissions",
            ),
            ("CRAWLER_REVIEW_AGENT_REVIEWER_EFFORT", "extreme"),
            ("CRAWLER_REVIEW_AGENT_AUTHOR_MAX_TURNS", "0"),
            ("CRAWLER_REVIEW_AGENT_AUTHOR_MAX_BUDGET_USD", "-1"),
            ("CRAWLER_REVIEW_AGENT_REVIEWER_MAX_BUDGET_USD", "NaN"),
            (MAX_PROCESSES_ENV, "two"),
            (WEEKLY_BUDGET_USD_ENV, "1e9"),
            (INTERVAL_SECONDS_ENV, "0"),
        ] {
            match settings(&[], &[(name, value)]) {
                Err(RunnerSettingsError::Invalid { name: named, .. }) => assert_eq!(named, name),
                other => panic!("{name}={value}: {other:?}"),
            }
        }
        assert!(matches!(
            settings(
                &[],
                &[("CRAWLER_REVIEW_CONSOLE_URL", "http://review.example")]
            ),
            Err(RunnerSettingsError::Console(_))
        ));
    }
}
