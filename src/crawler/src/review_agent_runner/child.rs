//! The runner as a child of `dev`: in `agent` authoring mode `dev` starts `review-agent-runner`
//! from next to its own executable as the service, the way systemd runs it in production, so a
//! development crawler works its reviews without a second terminal. It stays a separate process
//! with the same contract: it starts from an empty environment holding only the runner's own
//! settings and what its CLI children may take over, never the crawler's database URLs, cloud
//! credentials or the console's human token, and reaches the console only over HTTP with the
//! agents' token file, which `dev` writes to the directory the runner starts in.

use super::invocation::{PASSED_ENV, PASSED_PLATFORM_ENV};
use super::settings::{INTERVAL_SECONDS_ENV, STATE_DIR_ENV};
use super::stop::STOP_DEADLINE;
use crate::review_mcp::REVIEW_CONSOLE_URL_ENV;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// The runner's own settings: none holds a credential, as the agents' tokens are read from the
/// token file the runner is given by path.
const RUNNER_SETTING_PREFIX: &str = "CRAWLER_REVIEW_AGENT_";
/// The interval of `dev`'s runner unless set: an idle run costs one console request, and a
/// developer is waiting for the agents.
const DEV_INTERVAL_SECONDS: &str = "30";
/// Where `dev`'s runner keeps its lock, state, ledger and digests unless set, relative to `dev`'s
/// working directory: under the git-ignored build directory.
const DEV_STATE_DIR: &str = "target/review-agents";
/// How long the runner has to stop after `dev` asks it to: its own stop deadline, within which it
/// stops its agent processes and records their cost, and a margin.
const STOP_GRACE: Duration = Duration::from_secs(STOP_DEADLINE.as_secs() + 10);

/// The runner executable next to `executable`, where Cargo builds every binary of the crate.
pub fn runner_executable_next_to(executable: &Path) -> PathBuf {
    executable.with_file_name(if cfg!(windows) {
        "review-agent-runner.exe"
    } else {
        "review-agent-runner"
    })
}

/// The environment `dev` starts the runner with: of `dev`'s `vars`, the runner's settings
/// (`CRAWLER_REVIEW_AGENT_*`, `CRAWLER_REVIEW_CONSOLE_URL`, `LOG_LEVEL`) and the variables its CLI
/// children may take over ([`PASSED_ENV`], [`PASSED_PLATFORM_ENV`]), then `dev`'s defaults for
/// those of the console on `console_port`, the interval and the state directory that are unset or
/// empty. A variable that is not valid Unicode is left out; the runner could not read it.
pub fn runner_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    console_port: u16,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vars
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, value)| !value.trim().is_empty() && is_runner_variable(name))
        .collect();
    let console_url = format!("http://127.0.0.1:{console_port}");
    for (name, default) in [
        (REVIEW_CONSOLE_URL_ENV, console_url.as_str()),
        (INTERVAL_SECONDS_ENV, DEV_INTERVAL_SECONDS),
        (STATE_DIR_ENV, DEV_STATE_DIR),
    ] {
        if !env.iter().any(|(set, _)| same_name(set, name)) {
            env.push((name.to_owned(), default.to_owned()));
        }
    }
    env
}

fn is_runner_variable(name: &str) -> bool {
    let prefix_len = RUNNER_SETTING_PREFIX.len();
    name.get(..prefix_len)
        .is_some_and(|prefix| same_name(prefix, RUNNER_SETTING_PREFIX))
        || [REVIEW_CONSOLE_URL_ENV, "LOG_LEVEL"]
            .iter()
            .chain(PASSED_ENV.iter())
            .chain(PASSED_PLATFORM_ENV)
            .any(|passed| same_name(name, passed))
}

/// Windows reads variable names ignoring case, so `Path` is `PATH` there.
fn same_name(name: &str, other: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(other)
    } else {
        name == other
    }
}

/// The runner `dev` started; [`RunnerChild::stop`] stops it with `dev`.
pub struct RunnerChild {
    stop: oneshot::Sender<()>,
    supervisor: JoinHandle<()>,
}

impl RunnerChild {
    /// Starts `program` as the service in `working_dir` with only `env`; its JSON log joins
    /// `dev`'s output. A runner that cannot start, such as one that was not built, is logged and
    /// `dev` runs on without review agents; a runner that ends by itself, such as on the Ctrl+C
    /// that reaches `dev`'s whole terminal, is logged and not started again.
    pub fn start(program: &Path, working_dir: &Path, env: Vec<(String, String)>) -> Option<Self> {
        let mut command = Command::new(program);
        command
            .current_dir(working_dir)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                warn!(
                    program = %program.display(),
                    error = %error,
                    "Cannot start the review agent runner; dev runs without review agents. Build it with cargo build --locked -p crawler --bin dev --bin review-agent-runner --bin review-mcp"
                );
                return None;
            }
        };
        info!(
            program = %program.display(),
            pid = child.id(),
            "Started the review agent runner as a child of dev"
        );
        let (stop, stopped) = oneshot::channel();
        let supervisor = tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => warn!(
                    exit_code = status.ok().and_then(|status| status.code()),
                    "The review agent runner ended; dev starts no new one"
                ),
                _ = stopped => terminate(&mut child).await,
            }
        });
        Some(Self { stop, supervisor })
    }

    /// Stops the runner and waits until it ended.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.supervisor.await;
    }
}

/// Asks the runner to stop as `systemctl stop` does, with SIGTERM to it alone, so it stops its
/// agent processes and records their cost, and kills it if it has not ended after [`STOP_GRACE`].
/// After a Ctrl+C in `dev`'s terminal, which reaches the runner too, the runner is stopping already
/// and ignores this SIGTERM.
/// Windows has no SIGTERM: a Ctrl+C in `dev`'s console reaches the runner too, and a runner still
/// running after the grace is killed.
async fn terminate(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|id| libc::pid_t::try_from(id).ok()) {
        // SAFETY: the child has not been reaped, so its pid still names it.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    if tokio::time::timeout(STOP_GRACE, child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect()
    }

    #[test]
    fn should_pass_the_runner_only_its_settings_and_the_cli_s_variables() {
        let env = runner_env(
            vars(&[
                ("PATH", "/usr/bin"),
                ("CLAUDE_CONFIG_DIR", "/home/dev/.claude"),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN",
                    "/usr/local/bin/claude",
                ),
                ("CRAWLER_REVIEW_AGENT_AUTHOR_EFFORT", "medium"),
                ("LOG_LEVEL", "debug"),
                ("CRAWLER_REVIEW_AUTH_TOKEN", "human-token"),
                ("CRAWLER_AUTHORING_MODE", "agent"),
                (
                    "LOCAL_DB_URL",
                    "postgres://postgres:postgres@localhost/crawler_demo",
                ),
                ("AWS_SECRET_ACCESS_KEY", "aws-secret"),
                ("GOOGLE_APPLICATION_CREDENTIALS", "/etc/gcp.json"),
                ("ANTHROPIC_API_KEY", "api-key"),
                ("CRAWLER_DEV_START_REVIEW_AGENT_RUNNER", "true"),
            ]),
            7979,
        );

        assert_eq!(
            env,
            [
                ("PATH", "/usr/bin"),
                ("CLAUDE_CONFIG_DIR", "/home/dev/.claude"),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN",
                    "/usr/local/bin/claude"
                ),
                ("CRAWLER_REVIEW_AGENT_AUTHOR_EFFORT", "medium"),
                ("LOG_LEVEL", "debug"),
                ("CRAWLER_REVIEW_CONSOLE_URL", "http://127.0.0.1:7979"),
                ("CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS", "30"),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR",
                    "target/review-agents"
                ),
            ]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
    }

    #[test]
    fn should_keep_the_developer_s_runner_settings_over_dev_s_defaults() {
        let env = runner_env(
            vars(&[
                ("CRAWLER_REVIEW_CONSOLE_URL", "http://127.0.0.1:7878"),
                ("CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS", "5"),
                ("CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR", " "),
            ]),
            7979,
        );

        assert_eq!(
            env,
            [
                ("CRAWLER_REVIEW_CONSOLE_URL", "http://127.0.0.1:7878"),
                ("CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS", "5"),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR",
                    "target/review-agents"
                ),
            ]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
    }

    /// `dev`'s stop reaches the runner as SIGTERM, which it answers by stopping its agent
    /// processes, and the runner sees none of the test process's own variables.
    #[cfg(unix)]
    #[tokio::test]
    async fn should_start_the_runner_with_only_its_environment_and_stop_it_with_sigterm() {
        use crate::review_agent_runner::ledger::tests::StateDirectory;
        use std::os::unix::fs::PermissionsExt;

        let directory = StateDirectory::new();
        let record = directory.path().join("record");
        let program = directory.path().join("review-agent-runner");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nenv > '{record}'\ntrap 'echo stopped >> \"{record}\"; exit 143' TERM\nwhile :; do sleep 0.1; done\n",
                record = record.display()
            ),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());

        let runner = RunnerChild::start(
            &program,
            directory.path(),
            vec![("LOG_LEVEL".to_owned(), "info".to_owned())],
        )
        .unwrap_or_else(|| panic!("the runner did not start"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        tokio::time::timeout(STOP_GRACE / 2, runner.stop())
            .await
            .unwrap_or_else(|_| panic!("the runner was killed, not stopped"));

        let recorded = std::fs::read_to_string(&record).unwrap_or_default();
        assert!(recorded.contains("LOG_LEVEL=info"), "{recorded}");
        assert!(!recorded.contains("CARGO_PKG_NAME"), "{recorded}");
        assert!(recorded.ends_with("stopped\n"), "{recorded}");
    }
}
