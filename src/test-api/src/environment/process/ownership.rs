//! Recover the complete invocation even when a runner starts new process sessions.
use super::super::{Result, context, invalid};
use super::{CONFIG, MANAGED};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

struct Process {
    descriptor: OwnedFd,
}

/// PID descriptors fence reuse between ownership verification and signalling.
fn open_process(pid: i32) -> Result<Option<Process>> {
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(context("cannot retain managed process identity", error))
        };
    }
    Ok(Some(Process {
        descriptor: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
    }))
}

fn owns(environment: &[u8], marker: &[u8]) -> bool {
    let values: Vec<_> = environment.split(|byte| *byte == 0).collect();
    values.contains(&marker) && values.contains(&format!("{MANAGED}=1").as_bytes())
}

fn exited(path: &Path) -> bool {
    match std::fs::read_to_string(path.join("stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, fields)| fields.split_whitespace().next() == Some("Z")),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn invocation_processes(marker: &[u8], group: Option<u32>) -> Result<Vec<Process>> {
    let mut processes = vec![];
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        if fields.split_whitespace().next() == Some("Z") {
            continue;
        }
        let original_group = group.is_some_and(|group| {
            fields
                .split_whitespace()
                .nth(2)
                .and_then(|value| value.parse().ok())
                == Some(group)
        });
        let environment = match std::fs::read(entry.path().join("environ")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if original_group => {
                return Err(context("cannot verify stale child group ownership", error));
            }
            Err(_) => continue,
        };
        if !owns(&environment, marker) {
            if original_group && !exited(&entry.path()) {
                return Err(invalid("stale child group ownership could not be verified"));
            }
            continue;
        }
        let Some(process) = open_process(pid)? else {
            continue;
        };
        // Verify again after acquiring the PID descriptor. An exited PID can be
        // reused between the first /proc read and pidfd_open.
        match std::fs::read(entry.path().join("environ")) {
            Ok(environment) if owns(&environment, marker) => processes.push(process),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(context("cannot verify managed process identity", error)),
        }
    }
    Ok(processes)
}

fn kill(process: &Process) -> Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            process.descriptor.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(context("cannot stop managed invocation process", error));
        }
    }
    Ok(())
}

pub(super) fn terminate_invocation(dir: &Path, group: Option<u32>) -> Result<()> {
    if group.is_some_and(|group| group <= 1 || group > i32::MAX as u32) {
        return Err(invalid("refusing invalid child process group"));
    }
    let marker = format!("{CONFIG}={}", dir.join("environment.json").display());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut signalled: Vec<Process> = vec![];
    loop {
        let processes = invocation_processes(marker.as_bytes(), group)?;
        if processes.is_empty() {
            // A live supervisor is a subreaper. Reap only the exact children we
            // signalled; GC may instead find them parented to init (ECHILD).
            for process in signalled {
                let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
                let result = unsafe {
                    libc::waitid(
                        libc::P_PIDFD,
                        process.descriptor.as_raw_fd() as libc::id_t,
                        status.as_mut_ptr(),
                        libc::WEXITED | libc::WNOHANG,
                    )
                };
                if result != 0 {
                    let error = std::io::Error::last_os_error();
                    if !matches!(error.raw_os_error(), Some(libc::ECHILD | libc::ESRCH)) {
                        return Err(context("cannot reap managed invocation process", error));
                    }
                }
            }
            return Ok(());
        }
        for process in processes {
            kill(&process)?;
            signalled.push(process);
        }
        if Instant::now() >= deadline {
            return Err(invalid(
                "managed invocation processes did not stop before cleanup deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    struct Probe(Child, Process);
    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = kill(&self.1);
            let _ = self.0.wait();
        }
    }

    fn sleeper(dir: &Path) -> Probe {
        let child = Command::new("sleep")
            .arg("30")
            .env(MANAGED, "1")
            .env(CONFIG, dir.join("environment.json"))
            .process_group(0)
            .spawn()
            .unwrap();
        let process = open_process(child.id() as i32).unwrap().unwrap();
        Probe(child, process)
    }

    #[test]
    fn should_stop_every_owned_group_and_preserve_an_unrelated_invocation() {
        let dir = std::env::temp_dir().join(format!(
            "aura-owned-process-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let first = sleeper(&dir);
        let detached = sleeper(&dir);
        let mut foreign = sleeper(&dir.with_extension("foreign"));
        terminate_invocation(&dir, Some(first.0.id())).unwrap();
        assert!(!Path::new(&format!("/proc/{}", first.0.id())).exists());
        assert!(!Path::new(&format!("/proc/{}", detached.0.id())).exists());
        assert!(foreign.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn should_recover_a_marked_child_before_the_group_file_was_published() {
        let dir = std::env::temp_dir().join(format!(
            "aura-unpublished-process-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let child = sleeper(&dir);
        terminate_invocation(&dir, None).unwrap();
        assert!(!Path::new(&format!("/proc/{}", child.0.id())).exists());
    }

    #[tokio::test]
    async fn should_stop_a_detached_worker_when_the_supervised_command_finishes() {
        let dir = std::env::temp_dir().join(format!(
            "aura-normal-process-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut child = tokio::process::Command::new("sleep")
            .arg("0")
            .env(MANAGED, "1")
            .env(CONFIG, dir.join("environment.json"))
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let detached = sleeper(&dir);
        let group = child.id().unwrap();
        assert!(child.wait().await.unwrap().success());
        super::super::terminate(&mut child, group, &dir, libc::SIGTERM)
            .await
            .unwrap();
        assert!(!Path::new(&format!("/proc/{}", detached.0.id())).exists());
    }

    #[test]
    fn should_reject_a_reused_group_without_signalling_its_foreign_owner() {
        let dir = std::env::temp_dir().join(format!(
            "aura-reused-process-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut foreign = sleeper(&dir.with_extension("foreign"));
        assert!(terminate_invocation(&dir, Some(foreign.0.id())).is_err());
        assert!(foreign.0.try_wait().unwrap().is_none());
    }
}
