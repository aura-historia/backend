//! Running a CLI child: an agent process from its [`AgentCommand`], in a fresh empty working
//! directory of its own that is removed afterwards, or the login probe. The child starts from an
//! empty environment with only its [`ChildEnvironment`]: the few variables the CLI needs, taken
//! from the runner's, plus the variables a pinned CLI runs with. The end of its stdout is kept up
//! to a bound, where an agent process's stream ends with its result envelope, and its stderr only
//! as a short tail for a failure's log line. A child that outlives its timeout, or runs when the
//! runner stops, is stopped with everything it started, such as its `review-mcp`: first asked to
//! end its turn, which the CLI ends with its result envelope, and its output is what it printed
//! until it stopped, so the runner still reads a cost it reported.

use super::invocation::{AgentCommand, ChildEnvironment};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStderr, ChildStdout, Command};

/// The most of a child's stdout, and of its stderr, the runner keeps: the end of it. An agent
/// process's stream ends with its result envelope, a few kilobytes after what the agent read.
const MAX_OUTPUT_TAIL_BYTES: usize = 4 * 1024 * 1024;
/// How much of a child's output one read takes.
const READ_CHUNK_BYTES: usize = 16 * 1024;
/// The most stderr of a child the runner keeps, its tail.
const MAX_STDERR_TAIL_CHARS: usize = 300;
/// How long a stopped child has to end its turn after SIGINT, and then its process group to exit
/// after SIGTERM before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// The longest stopping a child takes: a grace after SIGINT and one after SIGTERM.
pub const LONGEST_CHILD_STOP: Duration = Duration::from_secs(2 * STOP_GRACE.as_secs());

/// What a finished or stopped child left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildOutput {
    /// The exit code, `None` when a signal ended it.
    pub exit_code: Option<i32>,
    /// The end of its stdout, up to when it ended or was stopped.
    pub stdout: Vec<u8>,
    /// The last line of its stderr, shortened, for a failure's log line.
    pub stderr_tail: String,
    /// Why the runner stopped it before it ended, if it did.
    pub stopped: Option<ChildStop>,
}

/// Why the runner stopped a child before it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStop {
    /// It outlived its timeout.
    Timeout,
    /// It ran when the runner stopped.
    RunnerStop,
}

#[derive(Debug, thiserror::Error)]
pub enum ChildError {
    #[error("cannot create the agent process's working directory {}", path.display())]
    WorkingDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot start {}", program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot talk to the child process")]
    Io(#[source] std::io::Error),
}

/// Runs the agent process `command` with its prompt on stdin, in a new empty directory, for at
/// most `timeout`, or until `runner_stop` resolves.
pub async fn run_agent(
    command: &AgentCommand,
    timeout: Duration,
    runner_stop: impl Future<Output = ()>,
) -> Result<ChildOutput, ChildError> {
    let working_dir = EmptyDirectory::create()?;
    let mut child = cli_command(&command.program, working_dir.path(), &command.env);
    child.args(&command.args);
    run(
        child,
        Some(command.prompt.as_bytes()),
        &command.program,
        timeout,
        runner_stop,
    )
    .await
}

/// Runs the CLI with `args` and no input, in a new empty directory with `env`, for at most
/// `timeout`. The runner's stop does not end it: it spends nothing, and the stop's deadline bounds
/// it.
pub async fn run_cli(
    program: &Path,
    args: &[&str],
    env: &ChildEnvironment,
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let working_dir = EmptyDirectory::create()?;
    let mut child = cli_command(program, working_dir.path(), env);
    child.args(args);
    run(child, None, program, timeout, std::future::pending()).await
}

fn cli_command(program: &Path, working_dir: &Path, env: &ChildEnvironment) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(working_dir)
        .env_clear()
        .envs(env.vars());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
        // SAFETY: between fork and exec the closure only calls `signal`, which is
        // async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                // A runner started with SIGINT ignored, as a shell starts a background job, must not
                // hand that on: a stopped child is asked with SIGINT to end its turn.
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
    }
    command
}

async fn run(
    mut command: Command,
    input: Option<&[u8]>,
    program: &Path,
    timeout: Duration,
    runner_stop: impl Future<Output = ()>,
) -> Result<ChildOutput, ChildError> {
    let mut child = command.spawn().map_err(|source| ChildError::Spawn {
        program: program.to_path_buf(),
        source,
    })?;
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let process_group = child.id();
    // Declared after `child`, so it is dropped first, while the group's leader is not yet reaped.
    let mut group_guard = ProcessGroupGuard(process_group);
    let mut output = OutputTails::default();
    let exchange = async {
        if let (Some(stdin), Some(input)) = (stdin.as_mut(), input) {
            // A child that exits without reading its input closes the pipe; its output says why.
            let _ = stdin.write_all(input).await;
        }
        drop(stdin.take());
        let (read, status) = tokio::join!(output.read_from(&mut stdout, &mut stderr), child.wait());
        read?;
        status
    };
    let finished = tokio::select! {
        finished = exchange => Ok(finished),
        () = tokio::time::sleep(timeout) => Err(ChildStop::Timeout),
        () = runner_stop => Err(ChildStop::RunnerStop),
    };
    let (exit_code, stopped) = match finished {
        Ok(Ok(status)) => (status.code(), None),
        Ok(Err(error)) => return Err(ChildError::Io(error)),
        Err(stop) => {
            stop_process_group(process_group, output.read_from(&mut stdout, &mut stderr)).await;
            let _ = child.kill().await;
            (None, Some(stop))
        }
    };
    group_guard.disarm();
    Ok(output.into_output(exit_code, stopped))
}

/// The process group of a child that has not ended yet. Dropping it kills the whole group: a run
/// that is cancelled, such as by the runner's stop signal, or fails while its child runs, leaves
/// nothing the child started, such as its `review-mcp`.
struct ProcessGroupGuard(Option<u32>);

impl ProcessGroupGuard {
    /// The child ended or its group was stopped: nothing is left to kill.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.0.and_then(|id| libc::pid_t::try_from(id).ok()) {
            // SAFETY: killpg only sends a signal to the group the runner created for this child,
            // whose leader it has not reaped yet, so the group id cannot have been reused.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}

/// The ends of a child's stdout and stderr, as far as they were read.
#[derive(Debug, Default)]
struct OutputTails {
    stdout: OutputTail,
    stderr: OutputTail,
}

impl OutputTails {
    /// Reads both streams to their ends. A read that is stopped keeps every byte it read before,
    /// so reading on later loses nothing.
    async fn read_from(
        &mut self,
        stdout: &mut Option<ChildStdout>,
        stderr: &mut Option<ChildStderr>,
    ) -> std::io::Result<()> {
        let (stdout, stderr) =
            tokio::join!(self.stdout.read_from(stdout), self.stderr.read_from(stderr));
        stdout.and(stderr)
    }

    fn into_output(self, exit_code: Option<i32>, stopped: Option<ChildStop>) -> ChildOutput {
        ChildOutput {
            exit_code,
            stdout: self.stdout.into_bytes(),
            stderr_tail: stderr_tail(&self.stderr.into_bytes()),
            stopped,
        }
    }
}

/// The end of a child's output: at most [`MAX_OUTPUT_TAIL_BYTES`], earlier bytes dropped as later
/// ones come, so a child that writes more is never blocked on a full pipe until its timeout.
#[derive(Debug, Default)]
struct OutputTail(VecDeque<u8>);

impl OutputTail {
    /// Reads `stream`, when there is one, to its end.
    async fn read_from(
        &mut self,
        stream: &mut Option<impl AsyncRead + Unpin>,
    ) -> std::io::Result<()> {
        let Some(stream) = stream else {
            return Ok(());
        };
        let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            self.0.extend(&chunk[..read]);
            let excess = self.0.len().saturating_sub(MAX_OUTPUT_TAIL_BYTES);
            self.0.drain(..excess);
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.0.into()
    }
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let last = text.lines().rev().find(|line| !line.trim().is_empty());
    last.map(|line| line.trim().chars().take(MAX_STDERR_TAIL_CHARS).collect())
        .unwrap_or_default()
}

/// Stops the child, the CLI, and its process group with the `review-mcp` the CLI started, while
/// `read_on` keeps what the child prints as it stops. First SIGINT to the CLI alone: it ends its
/// turn and prints its result envelope with the cost so far, which SIGTERM would leave unreported
/// (code.claude.com/docs/en/headless, "Stop a run with SIGTERM"). Once the child closed its
/// output, or after a grace period, the group gets SIGTERM, and SIGKILL after another.
async fn stop_process_group(process_group: Option<u32>, read_on: impl Future) {
    #[cfg(unix)]
    if let Some(group) = process_group.and_then(|id| libc::pid_t::try_from(id).ok()) {
        // A read that ended returns no more; one that fails or outlasts its grace keeps what it
        // read so far. The read never reaps the child, so the group id stays the child's.
        let read_on = futures::FutureExt::fuse(read_on);
        tokio::pin!(read_on);
        // SAFETY: the group's id is the child's process id, which the runner has not reaped.
        unsafe {
            libc::kill(group, libc::SIGINT);
        }
        let _ = tokio::time::timeout(STOP_GRACE, &mut read_on).await;
        // SAFETY: killpg only sends a signal to the group the runner created for this child.
        unsafe {
            libc::killpg(group, libc::SIGTERM);
        }
        let _ = tokio::join!(
            tokio::time::timeout(STOP_GRACE, &mut read_on),
            tokio::time::sleep(STOP_GRACE)
        );
        // SAFETY: as above; the group may be gone already, which killpg reports and we ignore.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (process_group, STOP_GRACE);
        drop(read_on);
    }
}

/// A new empty directory of its own under the system's temporary directory, outside any project,
/// so the CLI finds no project settings or instructions in it or its parents; removed on drop.
struct EmptyDirectory(PathBuf);

impl EmptyDirectory {
    fn create() -> Result<Self, ChildError> {
        let path = std::env::temp_dir().join(format!(
            "crawler-review-agent-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&path)
            .map_err(|source| ChildError::WorkingDirectory {
                path: path.clone(),
                source,
            })?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for EmptyDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::review_agent_runner::ledger::tests::StateDirectory;
    use std::os::unix::fs::PermissionsExt;

    fn script(directory: &StateDirectory, body: &str) -> PathBuf {
        let path = directory.path().join("child.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap_or_default();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        path
    }

    /// The environment the runner hands a child: the test process's `PATH`, so the script finds
    /// its tools, and one passed variable.
    fn child_env() -> ChildEnvironment {
        let path = std::env::var("PATH").unwrap_or_default();
        ChildEnvironment::from_env(|name| match name {
            "PATH" => Some(path.clone()),
            "LANG" => Some("C.UTF-8".to_owned()),
            _ => None,
        })
    }

    fn command(program: PathBuf) -> AgentCommand {
        AgentCommand {
            program,
            args: vec!["-p".to_owned(), "--tools".to_owned(), String::new()],
            env: child_env(),
            prompt: "Work review cr_1.".to_owned(),
        }
    }

    /// The child sees only its [`ChildEnvironment`]: none of the test process's own variables, such
    /// as Cargo's, reach it.
    #[tokio::test]
    async fn should_start_the_child_in_an_empty_directory_with_only_its_environment() {
        let directory = StateDirectory::new();
        let record = directory.path().join("record");
        let program = script(
            &directory,
            &format!(
                "{{ pwd; ls -A | wc -l; printf '%s|' \"$@\"; echo; cat; echo; env; }} > '{}'; echo '{{\"type\":\"result\"}}'; echo 'last words' >&2",
                record.display()
            ),
        );
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());

        let output = run_agent(
            &command(program),
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        let recorded = std::fs::read_to_string(&record).unwrap_or_default();
        let lines: Vec<&str> = recorded.lines().collect();
        assert!(lines[0].contains("crawler-review-agent-"), "{recorded}");
        assert!(
            !Path::new(lines[0]).exists(),
            "the working directory is removed"
        );
        assert_eq!(lines[1].trim(), "0");
        assert_eq!(lines[2], "-p|--tools||");
        assert_eq!(lines[3], "Work review cr_1.");
        let mut names: Vec<&str> = lines[4..]
            .iter()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .filter(|name| !["PWD", "SHLVL", "_", "OLDPWD"].contains(name))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
                "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
                "DISABLE_AUTOUPDATER",
                "LANG",
                "PATH",
                "TZ",
            ],
            "{recorded}"
        );
        assert!(recorded.contains("\nTZ=UTC\n"), "{recorded}");
        assert!(recorded.contains("\nLANG=C.UTF-8\n"), "{recorded}");
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.stdout, b"{\"type\":\"result\"}\n");
        assert_eq!(output.stderr_tail, "last words");
        assert_eq!(output.stopped, None);
    }

    /// A run that is dropped before its child ended, as the runner's stop signal drops it, stops
    /// the child together with what the child started.
    #[tokio::test]
    async fn should_stop_what_the_child_started_when_the_run_is_cancelled() {
        let directory = StateDirectory::new();
        let marker = directory.path().join("grandchild-alive");
        let program = script(
            &directory,
            &format!("(sleep 3; touch '{}') & sleep 30", marker.display()),
        );

        let cancelled = tokio::time::timeout(
            Duration::from_millis(500),
            run_agent(
                &command(program),
                Duration::from_secs(60),
                std::future::pending(),
            ),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert!(
            cancelled.is_err(),
            "the run was still waiting for its child"
        );
        assert!(!marker.exists(), "the child's own child was stopped too");
    }

    /// A chatty child finishes, and the runner keeps the end of what it printed, where an agent
    /// process's stream has its result envelope.
    #[tokio::test]
    async fn should_keep_the_end_of_a_chatty_child_s_output() {
        let directory = StateDirectory::new();
        let program = script(
            &directory,
            &format!(
                "head -c {} /dev/zero; echo; echo '{{\"type\":\"result\"}}'; echo done >&2",
                MAX_OUTPUT_TAIL_BYTES * 2
            ),
        );

        let output = run_agent(
            &command(program),
            Duration::from_secs(30),
            std::future::pending(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(output.stopped, None);
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.stdout.len(), MAX_OUTPUT_TAIL_BYTES);
        assert!(output.stdout.ends_with(b"\n{\"type\":\"result\"}\n"));
        assert_eq!(output.stderr_tail, "done");
    }

    /// A stopped child's output is what it printed until it ended: before its timeout, and while
    /// it stops. SIGINT comes first, on which the CLI ends its turn with its result envelope; a
    /// child that ignores it is asked again with SIGTERM.
    #[tokio::test]
    async fn should_keep_what_a_stopped_child_printed_before_and_while_it_stopped() {
        let directory = StateDirectory::new();
        let ends_its_turn = script(
            &directory,
            r#"trap 'echo "{\"type\":\"result\",\"total_cost_usd\":0.7}"; exit 130' INT
echo '{"type":"system","subtype":"init"}'
while :; do sleep 0.1; done"#,
        );
        let other = StateDirectory::new();
        let ends_on_sigterm = script(
            &other,
            r#"trap '' INT
trap 'echo "{\"type\":\"result\",\"total_cost_usd\":0.7}"; exit 143' TERM
echo '{"type":"system","subtype":"init"}'
while :; do sleep 0.1; done"#,
        );

        let (ends_its_turn, ends_on_sigterm) = (command(ends_its_turn), command(ends_on_sigterm));

        let (ended_its_turn, ended_on_sigterm) = tokio::join!(
            run_agent(
                &ends_its_turn,
                Duration::from_millis(500),
                std::future::pending()
            ),
            run_agent(
                &ends_on_sigterm,
                Duration::from_millis(500),
                std::future::pending()
            ),
        );

        for output in [ended_its_turn, ended_on_sigterm] {
            let output = output.unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(output.stopped, Some(ChildStop::Timeout));
            assert_eq!(output.exit_code, None);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "{\"type\":\"system\",\"subtype\":\"init\"}\n{\"type\":\"result\",\"total_cost_usd\":0.7}\n"
            );
        }
    }

    #[tokio::test]
    async fn should_stop_a_child_that_outlives_its_timeout_with_what_it_started() {
        let directory = StateDirectory::new();
        let marker = directory.path().join("grandchild-alive");
        let program = script(
            &directory,
            &format!("(sleep 8; touch '{}') & sleep 30", marker.display()),
        );

        let output = run_agent(
            &command(program),
            Duration::from_millis(300),
            std::future::pending(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        // The shell ends on SIGINT, its children only on SIGTERM, a grace period later, and the
        // stop returns two grace periods after the timeout.
        tokio::time::sleep(Duration::from_secs(1)).await;

        assert_eq!(output.stopped, Some(ChildStop::Timeout));
        assert!(!marker.exists(), "the child's own child was stopped too");
    }

    /// The runner's stop stops a child long before its timeout, as the timeout does: SIGINT first,
    /// on which the CLI prints its result envelope, then its process group with what it started.
    #[tokio::test]
    async fn should_stop_a_child_as_at_its_timeout_when_the_runner_stops() {
        let directory = StateDirectory::new();
        let marker = directory.path().join("grandchild-alive");
        let program = script(
            &directory,
            &format!(
                r#"trap 'echo "{{\"type\":\"result\",\"total_cost_usd\":0.3}}"; exit 130' INT
(sleep 8; touch '{}') >/dev/null 2>&1 &
while :; do sleep 0.1; done"#,
                marker.display()
            ),
        );

        let output = tokio::time::timeout(
            LONGEST_CHILD_STOP + Duration::from_secs(5),
            run_agent(
                &command(program),
                Duration::from_secs(3600),
                tokio::time::sleep(Duration::from_millis(300)),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("the stop waited for the child's timeout"))
        .unwrap_or_else(|error| panic!("{error}"));
        tokio::time::sleep(Duration::from_secs(1)).await;

        assert_eq!(output.stopped, Some(ChildStop::RunnerStop));
        assert_eq!(output.exit_code, None);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "{\"type\":\"result\",\"total_cost_usd\":0.3}\n"
        );
        assert!(!marker.exists(), "the child's own child was stopped too");
    }
}
