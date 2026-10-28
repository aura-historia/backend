//! Running a CLI child: an agent process from its [`AgentCommand`], in a fresh empty working
//! directory of its own that is removed afterwards, or the login probe. The child's environment
//! is the runner's without the variables that would take over billing or carry a credential, plus
//! the variables a pinned CLI runs with. Its stdout is read up to a bound and its stderr is kept
//! only as a short tail for a failure's log line. A child that outlives its timeout is stopped
//! with everything it started, such as its `review-mcp`.

use super::invocation::{AgentCommand, REMOVED_ENV, SET_ENV};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// The most stdout of a child the runner reads; an envelope is a few kilobytes.
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
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
    let mut child = cli_command(&command.program, working_dir.path());
    child.args(&command.args);
    run(
        child,
        Some(command.prompt.as_bytes()),
        &command.program,
        timeout,
    )
    .await
}

/// Runs the CLI with `args` and no input, in a new empty directory, for at most `timeout`.
pub async fn run_cli(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let working_dir = EmptyDirectory::create()?;
    let mut child = cli_command(program, working_dir.path());
    child.args(args);
    run(child, None, program, timeout).await
}

fn cli_command(program: &Path, working_dir: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(working_dir);
    for name in REMOVED_ENV {
        command.env_remove(name);
    }
    for (name, value) in SET_ENV {
        command.env(name, value);
    }
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
    let exchange = async {
        if let (Some(stdin), Some(input)) = (stdin.as_mut(), input) {
            // A child that exits without reading its input closes the pipe; its output says why.
            let _ = stdin.write_all(input).await;
        }
        drop(stdin.take());
        let (stdout, stderr, status) =
            tokio::join!(read_bounded(stdout), read_bounded(stderr), child.wait());
        Ok::<_, std::io::Error>((stdout?, stderr?, status?))
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(Ok((stdout, stderr, status))) => Ok(ChildOutput {
            exit_code: status.code(),
            stdout,
            stderr_tail: stderr_tail(&stderr),
            timed_out: false,
        }),
        Ok(Err(error)) => Err(ChildError::Io(error)),
        Err(_) => {
            stop_process_group(process_group).await;
            let _ = child.kill().await;
            Ok(ChildOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr_tail: String::new(),
                timed_out: true,
            })
        }
    }
}

async fn read_bounded(stream: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let Some(stream) = stream else {
        return Ok(bytes);
    };
    stream
        .take(u64::try_from(MAX_STDOUT_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
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

    fn command(program: PathBuf) -> AgentCommand {
        AgentCommand {
            program,
            args: vec!["-p".to_owned(), "--tools".to_owned(), String::new()],
            prompt: "Work review cr_1.".to_owned(),
        }
    }

    #[tokio::test]
    async fn should_start_the_child_in_an_empty_directory_with_the_cleaned_environment() {
        let directory = StateDirectory::new();
        let record = directory.path().join("record");
        let program = script(
            &directory,
            &format!(
                "{{ pwd; ls -A | wc -l; printf '%s|' \"$@\"; echo; cat; echo; env; }} > '{}'; echo '{{\"type\":\"result\"}}'; echo 'last words' >&2",
                record.display()
            ),
        );
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
        assert!(recorded.contains("DISABLE_AUTOUPDATER=1"));
        assert!(recorded.contains("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1"));
        assert!(recorded.contains("TZ=UTC"));
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.stdout, b"{\"type\":\"result\"}\n");
        assert_eq!(output.stderr_tail, "last words");
        assert!(!output.timed_out);
    }

    /// The process environment is shared by every test, so the removal is checked on the command
    /// rather than by setting the variables.
    #[test]
    fn should_remove_the_billing_and_console_credentials_from_the_child_s_environment() {
        let command = cli_command(Path::new("claude"), Path::new("/tmp"));

        let envs: Vec<(String, Option<String>)> = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CRAWLER_REVIEW_AUTH_TOKEN",
        ] {
            assert!(envs.contains(&(name.to_owned(), None)), "{name}: {envs:?}");
        }
        for (name, value) in [
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("TZ", "UTC"),
        ] {
            assert!(
                envs.contains(&(name.to_owned(), Some(value.to_owned()))),
                "{name}: {envs:?}"
            );
        }
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
