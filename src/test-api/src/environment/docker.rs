use super::{Result, context, invalid};
use std::ffi::OsStr;
use std::time::Duration;
use tokio::process::Command;

pub(super) const LABEL: &str = "io.aura-historia.test-env.v1";

/// The supervisor only talks to the local daemon, regardless of inherited Docker contexts.
pub(super) async fn docker<I, S>(args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("docker");
    command
        .args(["--host", "unix:///var/run/docker.sock"])
        .args(args)
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output())
        .await
        .map_err(|e| context("Docker operation timed out", e))?
        .map_err(|e| context("cannot invoke local Docker", e))?;
    if !output.status.success() {
        // Docker stderr can contain command configuration. Only the status is reported.
        return Err(context(
            "local Docker operation failed; inspect the run-owned resource state",
            DockerFailed(output.status),
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| context("Docker returned non-UTF-8 output", e))
}

pub(super) fn owner() -> String {
    format!("uid-{}", unsafe { libc::geteuid() })
}

pub(super) fn labels(run: &str) -> Vec<String> {
    vec![
        "--label".into(),
        format!("{LABEL}.owner={}", owner()),
        "--label".into(),
        format!("{LABEL}.run={run}"),
    ]
}

pub(super) async fn resource_ids(kind: &str, run: &str) -> Result<Vec<String>> {
    let mut args = match kind {
        "container" => vec!["ps", "-aq"],
        "volume" => vec!["volume", "ls", "-q"],
        "network" => vec!["network", "ls", "-q"],
        _ => return Err(invalid("unsupported owned resource kind")),
    };
    let run_filter = format!("label={LABEL}.run={run}");
    let owner_filter = format!("label={LABEL}.owner={}", owner());
    args.extend(["--filter", &run_filter, "--filter", &owner_filter]);
    Ok(docker(args).await?.lines().map(str::to_owned).collect())
}

pub(super) async fn cleanup(run: &str) -> Result<()> {
    let mut failure = None;
    for kind in ["container", "volume", "network"] {
        for id in resource_ids(kind, run).await? {
            let args = match kind {
                "container" => vec!["rm", "-f", "-v", &id],
                "volume" => vec!["volume", "rm", &id],
                _ => vec!["network", "rm", &id],
            };
            if let Err(error) = docker(args).await {
                failure.get_or_insert(error);
            }
        }
    }
    match failure {
        Some(error) => Err(context(
            "owned Docker cleanup failed; retain run directory and retry gc",
            error,
        )),
        None => Ok(()),
    }
}

pub(super) async fn published_port(name: &str, port: u16) -> Result<u16> {
    let value: serde_json::Value = serde_json::from_str(&docker(["inspect", name]).await?)
        .map_err(|e| context("cannot decode owned Docker port", e))?;
    let key = format!("{port}/tcp");
    let bindings = &value[0]["NetworkSettings"]["Ports"][key];
    if bindings[0]["HostIp"] != "127.0.0.1" {
        return Err(invalid("owned port is not bound exclusively to loopback"));
    }
    bindings[0]["HostPort"]
        .as_str()
        .ok_or_else(|| invalid("owned service has no host port"))?
        .parse()
        .map_err(|e| context("invalid owned service port", e))
}

pub(super) async fn generations(
    run: &str,
    expected: usize,
) -> Result<std::collections::BTreeMap<String, String>> {
    let ids = resource_ids("container", run).await?;
    if ids.len() != expected {
        return Err(invalid("required owned service disappeared"));
    }
    let mut generations = std::collections::BTreeMap::new();
    for id in ids {
        let value: serde_json::Value = serde_json::from_str(&docker(["inspect", &id]).await?)
            .map_err(|e| context("cannot decode owned Docker health", e))?;
        let state = &value[0]["State"];
        if state["Running"] != true
            || value[0]["RestartCount"] != 0
            || state["Health"]["Status"] == "unhealthy"
        {
            eprintln!(
                "AURA_TEST_ENV {}",
                serde_json::json!({"event":"SERVICE_UNHEALTHY","run_id":run,
                "container_id":id,"running":state["Running"],"restart_count":value[0]["RestartCount"]})
            );
            return Err(invalid(
                "required test service exited, became unhealthy or crossed restart boundary",
            ));
        }
        let generation = state["StartedAt"]
            .as_str()
            .ok_or_else(|| invalid("owned service has no start identity"))?;
        generations.insert(id, generation.to_owned());
    }
    Ok(generations)
}

pub(super) async fn monitor(
    run: &str,
    expected: std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        if generations(run, expected.len()).await? != expected {
            eprintln!(
                "AURA_TEST_ENV {}",
                serde_json::json!({"event":"SERVICE_RESTART_BOUNDARY","run_id":run})
            );
            return Err(invalid(
                "required service crossed its process restart boundary",
            ));
        }
    }
}

#[derive(Debug)]
struct DockerFailed(std::process::ExitStatus);
impl std::fmt::Display for DockerFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Docker exited with {}", self.0)
    }
}
impl std::error::Error for DockerFailed {}
