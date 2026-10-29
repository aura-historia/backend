//! Running a CLI child: an agent process from its [`AgentCommand`], in a fresh empty working
//! directory of its own that is removed afterwards, or the login probe. The child starts from an
//! empty environment with only its [`ChildEnvironment`]: the few variables the CLI needs, taken
//! from the runner's, plus the variables a pinned CLI runs with. The end of its stdout is kept up
//! to a bound, where an agent process's stream ends with its result envelope, and its stderr only
//! as a short tail for a failure's log line. A child that outlives its timeout is stopped with
//! everything it started, such as its `review-mcp`.

use super::invocation::{AgentCommand, ChildEnvironment};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// The most of a child's stdout, and of its stderr, the runner keeps: the end of it. An agent
/// process's stream ends with its result envelope, a few kilobytes after what the agent read.
const MAX_OUTPUT_TAIL_BYTES: usize = 4 * 1024 * 1024;
/// How much of a child's output one read takes.
const READ_CHUNK_BYTES: usize = 16 * 1024;
/// The most stderr of a child the runner keeps, its tail.
const MAX_STDERR_TAIL_CHARS: usize = 300;
/// How long a stopped child's process group has to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// What a finished child left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildOutput {
    /// The exit code, `None` when a signal ended it.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    /// The last line of its stderr, shortened, for a failure's log line.
    pub stderr_tail: String,
    pub timed_out: bool,
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
/// most `timeout`.
pub async fn run_agent(
    command: &AgentCommand,
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let working_dir = EmptyDirectory::create()?;
    let mut child = cli_command(&command.program, working_dir.path(), &command.env);
    child.args(&command.args);
    run(
        child,
        Some(command.prompt.as_bytes()),
        &command.program,
        timeout,
    )
    .await
}

/// Runs the CLI with `args` and no input, in a new empty directory with `env`, for at most
/// `timeout`.
pub async fn run_cli(
    program: &Path,
    args: &[&str],
    env: &ChildEnvironment,
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let working_dir = EmptyDirectory::create()?;
    let mut child = cli_command(program, working_dir.path(), env);
    child.args(args);
    run(child, None, program, timeout).await
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
    command.process_group(0);
    command
}

async fn run(
    mut command: Command,
    input: Option<&[u8]>,
    program: &Path,
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let mut child = command.spawn().map_err(|source| ChildError::Spawn {
        program: program.to_path_buf(),
        source,
    })?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let process_group = child.id();
    // Declared after `child`, so it is dropped first, while the group's leader is not yet reaped.
    let mut group_guard = ProcessGroupGuard(process_group);
    let exchange = async {
        if let (Some(stdin), Some(input)) = (stdin.as_mut(), input) {
            // A child that exits without reading its input closes the pipe; its output says why.
            let _ = stdin.write_all(input).await;
        }
        drop(stdin.take());
        let (stdout, stderr, status) =
            tokio::join!(read_tail(stdout), read_tail(stderr), child.wait());
        Ok::<_, std::io::Error>((stdout?, stderr?, status?))
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(Ok((stdout, stderr, status))) => {
            group_guard.disarm();
            Ok(ChildOutput {
                exit_code: status.code(),
                stdout,
                stderr_tail: stderr_tail(&stderr),
                timed_out: false,
            })
        }
        Ok(Err(error)) => Err(ChildError::Io(error)),
        Err(_) => {
            stop_process_group(process_group).await;
            let _ = child.kill().await;
            group_guard.disarm();
            Ok(ChildOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr_tail: String::new(),
                timed_out: true,
            })
        }
    }
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

/// Reads `stream` to its end and keeps its last [`MAX_OUTPUT_TAIL_BYTES`].
async fn read_tail(stream: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut tail = OutputTail::default();
    if let Some(mut stream) = stream {
        tail.read_from(&mut stream).await?;
    }
    Ok(tail.into_bytes())
}

/// The end of a child's output: at most [`MAX_OUTPUT_TAIL_BYTES`], earlier bytes dropped as later
/// ones come, so a child that writes more is never blocked on a full pipe until its timeout.
#[derive(Debug, Default)]
struct OutputTail(VecDeque<u8>);

impl OutputTail {
    /// Reads `stream` to its end. A read that is stopped keeps every byte it read before.
    async fn read_from(&mut self, stream: &mut (impl AsyncRead + Unpin)) -> std::io::Result<()> {
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

/// Stops the child's process group, the CLI and the `review-mcp` it started: SIGTERM, then SIGKILL
/// after a grace period.
async fn stop_process_group(process_group: Option<u32>) {
    #[cfg(unix)]
    if let Some(group) = process_group.and_then(|id| libc::pid_t::try_from(id).ok()) {
        // SAFETY: killpg only sends a signal to the group the runner created for this child.
        unsafe {
            libc::killpg(group, libc::SIGTERM);
        }
        tokio::time::sleep(STOP_GRACE).await;
        // SAFETY: as above; the group may be gone already, which killpg reports and we ignore.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = (process_group, STOP_GRACE);
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

        let output = run_agent(&command(program), Duration::from_secs(30))
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
        assert!(!output.timed_out);
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
            run_agent(&command(program), Duration::from_secs(60)),
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

        let output = run_agent(&command(program), Duration::from_secs(30))
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(!output.timed_out);
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.stdout.len(), MAX_OUTPUT_TAIL_BYTES);
        assert!(output.stdout.ends_with(b"\n{\"type\":\"result\"}\n"));
        assert_eq!(output.stderr_tail, "done");
    }

    #[tokio::test]
    async fn should_stop_a_child_that_outlives_its_timeout_with_what_it_started() {
        let directory = StateDirectory::new();
        let marker = directory.path().join("grandchild-alive");
        let program = script(
            &directory,
            &format!("(sleep 3; touch '{}') & sleep 30", marker.display()),
        );

        let output = run_agent(&command(program), Duration::from_millis(300))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        tokio::time::sleep(Duration::from_secs(4)).await;

        assert!(output.timed_out);
        assert!(!marker.exists(), "the child's own child was stopped too");
    }
}
