//! How `review-agent-runner` is started: whether it runs every interval until stopped or once
//! (`--once`), and from its environment the review console, the CLI and `review-mcp` it starts, its
//! state directory, each agent role's model, effort and turn cap and its model on URL-pattern
//! reviews, the run's limits and the interval between runs. The limits of the agents' decisions are the console's, which each run
//! reads from it; the runner reads none from its environment.
//! An empty or whitespace-only setting counts as unset; any other value that is not accepted fails
//! startup with the variable's name, so a mistyped limit never silently widens what the agents
//! may do.

use super::invocation::ChildEnvironment;
use super::queue::ReviewArtifact;
use super::usage_pacing::UsagePacing;
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
pub const INTERVAL_SECONDS_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS";
pub const FIVE_HOUR_SHARE_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_FIVE_HOUR_SHARE_PERCENT";
pub const WEEKLY_SHARE_ENV: &str = "CRAWLER_REVIEW_AGENT_RUNNER_WEEKLY_SHARE_PERCENT";
/// The model of an author process on a URL-pattern review, in place of the author's model, for
/// the experiment of #2835 whether a cheaper author induces a pattern as well. Unset, a URL-pattern
/// review gets the author's model.
pub const AUTHOR_URL_PATTERN_MODEL_ENV: &str = "CRAWLER_REVIEW_AGENT_AUTHOR_URL_PATTERN_MODEL";
/// The model of a reviewer process on a URL-pattern review, in place of the reviewer's model.
pub const REVIEWER_URL_PATTERN_MODEL_ENV: &str = "CRAWLER_REVIEW_AGENT_REVIEWER_URL_PATTERN_MODEL";

/// What each role runs with unless its settings say otherwise: the configuration that showed no
/// finding in the agent comparison of #2835 at the lowest cost, a Sonnet-class author and a
/// Haiku-class reviewer, both at `medium` effort. A URL-pattern review gets a Sonnet-class
/// reviewer: it judges from the URLs alone whether a pattern accounts for every product route
/// family of the sample and collides with no other route.
const AUTHOR_DEFAULTS: RoleDefaults = RoleDefaults {
    model: "claude-sonnet-5-5",
    url_pattern_model: None,
    effort: ClaudeEffort::Medium,
    max_turns: NonZeroU32::new(80).unwrap(),
};
const REVIEWER_DEFAULTS: RoleDefaults = RoleDefaults {
    model: "claude-haiku-5-5",
    url_pattern_model: Some("claude-sonnet-5-5"),
    effort: ClaudeEffort::Medium,
    max_turns: NonZeroU32::new(50).unwrap(),
};
const DEFAULT_MAX_REVIEWS_PER_RUN: NonZeroU32 = NonZeroU32::new(60).unwrap();
/// The console's default cap on live jobs: a further process would only wait for one, since each
/// agent process holds at most one live job at a time.
const DEFAULT_MAX_PROCESSES: NonZeroU32 = NonZeroU32::new(20).unwrap();
const DEFAULT_COOLDOWN_MINUTES: NonZeroU32 = NonZeroU32::new(180).unwrap();
const DEFAULT_PROCESS_TIMEOUT_MINUTES: NonZeroU32 = NonZeroU32::new(60).unwrap();
const DEFAULT_INTERVAL_SECONDS: NonZeroU32 = NonZeroU32::new(60).unwrap();
/// The share of the login's usage windows the agents take by default, leaving the rest to the
/// people who use the same login.
const DEFAULT_SHARE_PERCENT: u32 = 100;

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
    /// The model of an author process on a URL-pattern review when it is not the author's.
    pub author_url_pattern_model: Option<String>,
    pub reviewer: AgentProcessSettings,
    /// The model of a reviewer process on a URL-pattern review when it is not the reviewer's.
    pub reviewer_url_pattern_model: Option<String>,
    pub max_reviews_per_run: NonZeroU32,
    /// How many agent processes run at once.
    pub max_processes: NonZeroU32,
    /// How long a review that ended without a decision waits before the next pass.
    pub cooldown: Duration,
    /// How long one agent process may run before it is stopped.
    pub process_timeout: Duration,
    /// How long the service rests after a run ends before it starts the next; `--once` ignores it.
    pub interval: Duration,
    /// The shares of the login's five-hour and weekly usage windows the agents may take.
    pub pacing: UsagePacing,
}

impl RunnerSettings {
    pub fn role(&self, role: AgentRole) -> &AgentProcessSettings {
        match role {
            AgentRole::Author => &self.author,
            AgentRole::Reviewer => &self.reviewer,
        }
    }

    /// The model of `role` on a URL-pattern review in place of the role's model, if any.
    pub fn url_pattern_model(&self, role: AgentRole) -> Option<&str> {
        match role {
            AgentRole::Author => self.author_url_pattern_model.as_deref(),
            AgentRole::Reviewer => self.reviewer_url_pattern_model.as_deref(),
        }
    }

    /// What a process of `role` on a review of `artifact` is started with: the role's settings,
    /// on a URL-pattern review with the role's [`url_pattern_model`](Self::url_pattern_model) when
    /// it has one.
    pub fn process(&self, role: AgentRole, artifact: ReviewArtifact) -> AgentProcessSettings {
        let settings = self.role(role).clone();
        let model = match artifact {
            ReviewArtifact::ProductSchema => None,
            ReviewArtifact::UrlPattern => self.url_pattern_model(role),
        };
        match model {
            Some(model) => AgentProcessSettings {
                model: model.to_owned(),
                ..settings
            },
            None => settings,
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
            author: read.agent(AgentRole::Author, &AUTHOR_DEFAULTS)?,
            author_url_pattern_model: read
                .url_pattern_model(AgentRole::Author, &AUTHOR_DEFAULTS)?,
            reviewer: read.agent(AgentRole::Reviewer, &REVIEWER_DEFAULTS)?,
            reviewer_url_pattern_model: read
                .url_pattern_model(AgentRole::Reviewer, &REVIEWER_DEFAULTS)?,
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
            interval: Duration::from_secs(u64::from(
                read.positive(INTERVAL_SECONDS_ENV)?
                    .unwrap_or(DEFAULT_INTERVAL_SECONDS)
                    .get(),
            )),
            pacing: UsagePacing {
                five_hour_share: read.share(FIVE_HOUR_SHARE_ENV)?,
                weekly_share: read.share(WEEKLY_SHARE_ENV)?,
            },
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

/// A role's model, effort and turn cap when its settings name none.
struct RoleDefaults {
    model: &'static str,
    /// The role's model on a URL-pattern review; `None` runs it on `model`.
    url_pattern_model: Option<&'static str>,
    effort: ClaudeEffort,
    max_turns: NonZeroU32,
}

struct Reader<'a, F: Fn(&str) -> Option<String>> {
    setting: &'a F,
}

impl<F: Fn(&str) -> Option<String>> Reader<'_, F> {
    fn agent(
        &self,
        role: AgentRole,
        defaults: &RoleDefaults,
    ) -> Result<AgentProcessSettings, RunnerSettingsError> {
        let model = self
            .model(&role_setting(role, "MODEL"))?
            .unwrap_or_else(|| defaults.model.to_owned());
        let effort_name = role_setting(role, "EFFORT");
        let effort = match (self.setting)(&effort_name) {
            Some(effort) => parse_effort(&effort).ok_or_else(|| {
                invalid(effort_name, effort, "one of low, medium, high, xhigh, max")
            })?,
            None => defaults.effort,
        };
        Ok(AgentProcessSettings {
            model,
            effort,
            max_turns: self
                .positive(&role_setting(role, "MAX_TURNS"))?
                .unwrap_or(defaults.max_turns),
        })
    }

    /// The model of `role` on a URL-pattern review: [`AUTHOR_URL_PATTERN_MODEL_ENV`] or
    /// [`REVIEWER_URL_PATTERN_MODEL_ENV`], or else the role's default, `None` when neither names
    /// one.
    fn url_pattern_model(
        &self,
        role: AgentRole,
        defaults: &RoleDefaults,
    ) -> Result<Option<String>, RunnerSettingsError> {
        let name = match role {
            AgentRole::Author => AUTHOR_URL_PATTERN_MODEL_ENV,
            AgentRole::Reviewer => REVIEWER_URL_PATTERN_MODEL_ENV,
        };
        Ok(self
            .model(name)?
            .or_else(|| defaults.url_pattern_model.map(str::to_owned)))
    }

    /// The model alias or name `name` sets, `None` when it is unset.
    fn model(&self, name: &str) -> Result<Option<String>, RunnerSettingsError> {
        match (self.setting)(name) {
            Some(model) if is_model_name(&model) => Ok(Some(model)),
            Some(model) => Err(invalid(
                name.to_owned(),
                model,
                "a model alias or name of letters, digits, '.', '-', '_', '[' and ']' that starts with a letter or digit",
            )),
            None => Ok(None),
        }
    }

    /// A percentage from 1 to 100 as a share from 0.01 to 1, [`DEFAULT_SHARE_PERCENT`] unless set.
    fn share(&self, name: &str) -> Result<f64, RunnerSettingsError> {
        let percent = match (self.setting)(name) {
            Some(value) => value
                .parse::<u32>()
                .ok()
                .filter(|percent| (1..=100).contains(percent))
                .ok_or_else(|| invalid(name.to_owned(), value, "a whole number from 1 to 100"))?,
            None => DEFAULT_SHARE_PERCENT,
        };
        Ok(f64::from(percent) / 100.0)
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
        assert_eq!(read.interval, Duration::from_secs(60));
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
                model: "claude-sonnet-5-5".to_owned(),
                effort: ClaudeEffort::Medium,
                max_turns: NonZeroU32::new(80).unwrap_or(NonZeroU32::MIN),
            }
        );
        assert_eq!(
            read.reviewer,
            AgentProcessSettings {
                model: "claude-haiku-5-5".to_owned(),
                effort: ClaudeEffort::Medium,
                max_turns: NonZeroU32::new(50).unwrap_or(NonZeroU32::MIN),
            }
        );
        assert_eq!(read.max_reviews_per_run.get(), 60);
        assert_eq!(read.max_processes.get(), 20);
        assert_eq!(read.cooldown, Duration::from_secs(180 * 60));
        assert_eq!(read.process_timeout, Duration::from_secs(60 * 60));
        assert_eq!(
            read.pacing,
            UsagePacing {
                five_hour_share: 1.0,
                weekly_share: 1.0
            }
        );
    }

    #[test]
    fn should_read_each_role_s_settings_and_the_run_s_limits_from_the_environment() {
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
                (MAX_REVIEWS_PER_RUN_ENV, "9"),
                (MAX_PROCESSES_ENV, "1"),
                (COOLDOWN_MINUTES_ENV, "5"),
                (PROCESS_TIMEOUT_MINUTES_ENV, "2"),
                (INTERVAL_SECONDS_ENV, "30"),
                (FIVE_HOUR_SHARE_ENV, "50"),
                (WEEKLY_SHARE_ENV, "100"),
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
        assert_eq!(read.reviewer.model, "claude-haiku-5-5");
        assert_eq!(read.reviewer.max_turns.get(), 12);
        assert_eq!(read.max_reviews_per_run.get(), 9);
        assert_eq!(read.max_processes.get(), 1);
        assert_eq!(read.cooldown, Duration::from_secs(300));
        assert_eq!(read.process_timeout, Duration::from_secs(120));
        assert_eq!(read.interval, Duration::from_secs(30));
        assert_eq!(read.pacing.five_hour_share, 0.5);
        assert_eq!(read.pacing.weekly_share, 1.0);
    }

    /// By default the reviewer of a URL-pattern review runs on Sonnet and every other process on
    /// its role's model; each role's URL-pattern model replaces only the model, and only there.
    #[test]
    fn should_run_each_role_on_its_url_pattern_model_on_a_url_pattern_review_only() {
        let models = |settings: &RunnerSettings| {
            [
                (AgentRole::Author, ReviewArtifact::ProductSchema),
                (AgentRole::Author, ReviewArtifact::UrlPattern),
                (AgentRole::Reviewer, ReviewArtifact::ProductSchema),
                (AgentRole::Reviewer, ReviewArtifact::UrlPattern),
            ]
            .map(|(role, artifact)| settings.process(role, artifact).model)
        };
        let defaults = settings(&[], &[]).unwrap_or_else(|error| panic!("{error}"));
        let set = settings(
            &[],
            &[
                (AUTHOR_URL_PATTERN_MODEL_ENV, "claude-haiku-5-5"),
                (REVIEWER_URL_PATTERN_MODEL_ENV, "claude-opus-5-5"),
                ("CRAWLER_REVIEW_AGENT_REVIEWER_MODEL", "claude-sonnet-5-5"),
            ],
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            models(&defaults),
            [
                "claude-sonnet-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-sonnet-5-5"
            ]
        );
        assert_eq!(
            models(&set),
            [
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-sonnet-5-5",
                "claude-opus-5-5"
            ]
        );
        assert_eq!(
            set.process(AgentRole::Reviewer, ReviewArtifact::UrlPattern),
            AgentProcessSettings {
                model: "claude-opus-5-5".to_owned(),
                ..set.reviewer.clone()
            },
            "only the model changes"
        );
        assert_eq!(
            [AgentRole::Author, AgentRole::Reviewer]
                .map(|role| role_setting(role, "URL_PATTERN_MODEL")),
            [AUTHOR_URL_PATTERN_MODEL_ENV, REVIEWER_URL_PATTERN_MODEL_ENV]
        );
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
            (
                AUTHOR_URL_PATTERN_MODEL_ENV,
                "haiku --dangerously-skip-permissions",
            ),
            (REVIEWER_URL_PATTERN_MODEL_ENV, "-p"),
            ("CRAWLER_REVIEW_AGENT_REVIEWER_EFFORT", "extreme"),
            ("CRAWLER_REVIEW_AGENT_AUTHOR_MAX_TURNS", "0"),
            (MAX_PROCESSES_ENV, "two"),
            (INTERVAL_SECONDS_ENV, "0"),
            (FIVE_HOUR_SHARE_ENV, "0"),
            (WEEKLY_SHARE_ENV, "101"),
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
