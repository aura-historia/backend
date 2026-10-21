use super::contract::{CONFIG, MANAGED, ManagedEnvironment};
use super::{Result, context, invalid};
use std::ffi::OsString;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::signal::unix::{Signal, SignalKind, signal};

pub(super) struct Signals {
    interrupt: Signal,
    terminate: Signal,
}
impl Signals {
    pub(super) fn install() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }
    pub(super) async fn receive(&mut self) -> i32 {
        tokio::select! {_=self.interrupt.recv()=>libc::SIGINT,_=self.terminate.recv()=>libc::SIGTERM}
    }
}

pub(super) fn child_environment<I>(
    values: I,
    config: &ManagedEnvironment,
    path: &Path,
) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut values: Vec<_> = values
        .into_iter()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !key.starts_with("AWS_")
                && !key.starts_with("AURA_TEST_")
                && !key.starts_with("GOOGLE_")
                && !key.starts_with("GCP_")
                && !key.starts_with("STRIPE_")
                && !key.starts_with("LOOPS_")
                && !key.starts_with("LOCALSTACK_")
        })
        .collect();
    let credentials = path.with_file_name("empty-aws-config");
    for (key, value) in [
        (MANAGED, "1"),
        (
            CONFIG,
            path.to_str().expect("validated UTF-8 state directory"),
        ),
        ("AWS_ACCESS_KEY_ID", "test"),
        ("AWS_SECRET_ACCESS_KEY", "test"),
        ("AWS_SESSION_TOKEN", "test"),
        ("AWS_REGION", "eu-central-1"),
        ("AWS_DEFAULT_REGION", "eu-central-1"),
        ("AWS_EC2_METADATA_DISABLED", "true"),
        ("AWS_CONFIG_FILE", credentials.to_str().unwrap()),
        ("AWS_SHARED_CREDENTIALS_FILE", credentials.to_str().unwrap()),
    ] {
        values.push((key.into(), value.into()));
    }
    if let Some(endpoint) = config.aws_endpoint() {
        values.push(("AWS_ENDPOINT_URL".into(), endpoint.into()));
    }
    values
}

pub(super) fn enable_subreaper() -> Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(context(
            "cannot own orphaned test descendants",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

pub(super) fn spawn(
    command: &[OsString],
    config: &ManagedEnvironment,
    path: &Path,
) -> Result<Child> {
    let executable = command
        .first()
        .ok_or_else(|| invalid("run requires a child command after --"))?;
    let mut child = Command::new(executable);
    child
        .args(&command[1..])
        .env_clear()
        .envs(child_environment(std::env::vars_os(), config, path))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0)
        .kill_on_drop(true);
    child
        .spawn()
        .map_err(|e| context("cannot spawn supervised test command", e))
}

pub(super) fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

fn send(group: u32, signal: i32) -> Result<()> {
    let id: i32 = group
        .try_into()
        .map_err(|e| context("invalid child process group", e))?;
    if id <= 1 {
        return Err(invalid("refusing invalid child process group"));
    }
    if unsafe { libc::kill(-id, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(context("cannot signal owned child group", error));
        }
    }
    Ok(())
}

pub(super) async fn terminate(child: &mut Child, group: u32, signal: i32) -> Result<()> {
    send(group, signal)?;
    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            send(group, libc::SIGKILL)?;
            return Err(context("cannot wait for supervised command", error));
        }
        Err(_) => {
            send(group, libc::SIGKILL)?;
            tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .map_err(|e| context("supervised command did not exit after SIGKILL", e))??;
        }
    }
    // The leader can exit before its descendants. Remove the rest of this owned group too.
    send(group, libc::SIGKILL)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(-(group as i32), &mut status, libc::WNOHANG) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(context("cannot reap owned child descendants", error));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(invalid(
                "owned descendants did not exit before cleanup deadline",
            ));
        }
        if result == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// GC only signals groups whose remaining members carry this exact managed invocation.
pub(super) fn reap_stale_group(dir: &Path) -> Result<()> {
    let path = dir.join("child-group");
    if !path.exists() {
        return Ok(());
    }
    let group: u32 = std::fs::read_to_string(path)?.trim().parse()?;
    let marker = format!("{CONFIG}={}", dir.join("environment.json").display());
    let mut members = false;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_none_or(|s| s.parse::<u32>().is_err())
        {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        if fields
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse::<u32>().ok())
            != Some(group)
        {
            continue;
        }
        let environment = match std::fs::read(entry.path().join("environ")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(context("cannot verify stale child group ownership", error)),
        };
        // Zombies have no environment and cannot execute; leave their reaping to init.
        if fields.split_whitespace().next() == Some("Z") {
            continue;
        }
        if !environment
            .split(|b| *b == 0)
            .any(|v| v == marker.as_bytes())
        {
            return Err(invalid("stale child group ownership could not be verified"));
        }
        members = true;
    }
    if members {
        send(group, libc::SIGKILL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_replace_credential_sources_and_service_overrides_with_fixed_test_identity() {
        let config=ManagedEnvironment::from_json(br#"{"schema_version":1,"run_id":"0123456789abcdef0123456789abcdef","profile":"AWS","slots":4,"postgres":null,"aws_endpoint":"http://127.0.0.1:4567","opensearch_endpoint":null,"aws_services":["eventbridge","s3","sesv2","sqs"]}"#).unwrap();
        let original = [
            ("AWS_PROFILE", "canary"),
            ("AWS_ENDPOINT_URL_S3", "https://foreign.invalid"),
            ("AWS_WEB_IDENTITY_TOKEN_FILE", "canary"),
            ("AWS_ACCESS_KEY_ID", "canary"),
            ("PATH", "/bin"),
        ]
        .map(|(k, v)| (k.into(), v.into()));
        let values = child_environment(original, &config, Path::new("/tmp/run/environment.json"));
        assert!(
            !values
                .iter()
                .any(|(_, v)| v == "canary" || v == "https://foreign.invalid")
        );
        assert!(
            values
                .iter()
                .any(|(k, v)| k == "AWS_ACCESS_KEY_ID" && v == "test")
        );
        assert!(
            values
                .iter()
                .any(|(k, v)| k == "AWS_ENDPOINT_URL" && v == "http://127.0.0.1:4567")
        );
        assert!(values.iter().any(|(k, v)| k == "PATH" && v == "/bin"));
    }
}
