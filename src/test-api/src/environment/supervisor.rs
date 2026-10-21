use super::contract::{ManagedEnvironment, Profile, valid_run_id};
use super::{Result, context, docker, invalid, process, services};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema_version: u32,
    run_id: String,
    uid: u32,
}

fn directory(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(context("cannot create test supervisor state directory", e)),
    }
    validate_directory(path)
}

fn validate_directory(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(invalid(
            "supervisor state directory must be private and owned by the current user",
        ));
    }
    Ok(())
}

fn lock(dir: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(context("cannot lock test supervisor invocation", e))
        }
    }
}

fn lock_metadata(root: &Path) -> Result<File> {
    let file = File::open(root)?;
    file.lock()
        .map_err(|e| context("cannot lock test supervisor metadata", e))?;
    Ok(file)
}

pub(super) fn default_state_dir() -> PathBuf {
    std::env::temp_dir().join(format!("aura-test-env-v1-{}", unsafe { libc::geteuid() }))
}

struct StaleRun {
    run: String,
    dir: PathBuf,
    _lock: File,
}

fn run_directories(root: &Path) -> Result<Vec<PathBuf>> {
    directory(root)?;
    std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(Into::into))
        .collect()
}

fn claim_stale_run(root: &Path, dir: PathBuf) -> Result<Option<StaleRun>> {
    let Some(name) = dir.file_name().and_then(std::ffi::OsStr::to_str) else {
        return Ok(None);
    };
    let pending = name.strip_prefix(".pending-");
    let run = pending.unwrap_or(name);
    if !valid_run_id(run) {
        return Ok(None);
    }
    let metadata = lock_metadata(root)?;
    match validate_directory(&dir) {
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        result => result?,
    }
    let lock = match lock(&dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(None),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if pending.is_some() {
        if std::fs::read_dir(&dir)?.any(|entry| match entry {
            Ok(entry) => !matches!(entry.file_name().to_str(), Some("lock" | "owner.json")),
            Err(_) => true,
        }) {
            return Err(invalid("unexpected files in pending run initialization"));
        }
        std::fs::remove_dir_all(&dir)?;
        return Ok(None);
    }
    drop(metadata);
    let owner: Owner = serde_json::from_slice(&std::fs::read(dir.join("owner.json"))?)?;
    if owner.schema_version != 1 || owner.run_id != run || owner.uid != unsafe { libc::geteuid() } {
        return Err(invalid("stale run ownership manifest is invalid"));
    }
    process::reap_stale_group(&dir)?;
    Ok(Some(StaleRun {
        run: run.into(),
        dir,
        _lock: lock,
    }))
}

pub(super) async fn gc(root: &Path) -> Result<()> {
    for dir in run_directories(root)? {
        if let Some(stale) = claim_stale_run(root, dir)? {
            cleanup(&stale.run, &stale.dir).await?;
        }
    }
    Ok(())
}

async fn cleanup(run: &str, dir: &Path) -> Result<()> {
    let started = Instant::now();
    let result = match docker::cleanup(run).await {
        Ok(()) => remove_run_directory(dir),
        Err(error) => Err(error),
    };
    eprintln!(
        "AURA_TEST_ENV {}",
        serde_json::json!({"event":if result.is_ok(){"CLEANUP_COMPLETE"}else{"CLEANUP_FAILED"},
        "run_id":run,"elapsed_ms":started.elapsed().as_millis()})
    );
    result
}

fn remove_run_directory(dir: &Path) -> Result<()> {
    let root = dir
        .parent()
        .ok_or_else(|| invalid("owned run directory has no state root"))?;
    // Preserve the lock inode until GC can no longer discover this directory.
    let _metadata = lock_metadata(root)?;
    std::fs::remove_dir_all(dir).map_err(|e| context("cannot remove owned run directory", e))
}

fn initialize_run(root: &Path, run: &str) -> Result<(PathBuf, File)> {
    // GC shares this short metadata lock so it cannot collect a pending directory
    // before the run lock and ownership record are published.
    let _initialization = lock_metadata(root)?;
    let pending = root.join(format!(".pending-{run}"));
    directory(&pending)?;
    let lock = lock(&pending)?.ok_or_else(|| invalid("new run lock unexpectedly held"))?;
    let dir = root.join(run);
    let owner = Owner {
        schema_version: 1,
        run_id: run.into(),
        uid: unsafe { libc::geteuid() },
    };
    std::fs::write(pending.join("owner.json"), serde_json::to_vec(&owner)?)?;
    std::fs::rename(&pending, &dir)?;
    Ok((dir, lock))
}

pub(super) async fn run(
    profile: Profile,
    slots: u16,
    root: &Path,
    command: &[std::ffi::OsString],
) -> Result<i32> {
    if !(1..=1024).contains(&slots) || !root.is_absolute() || root.to_str().is_none() {
        return Err(invalid(
            "run requires 1..1024 slots and an absolute UTF-8 state directory",
        ));
    }
    if super::managed_environment().is_some() {
        return Err(invalid("nested managed supervisor is unsupported"));
    }
    process::enable_subreaper()?;
    let mut signals = process::Signals::install()?;
    gc(root).await?;
    let run = uuid::Uuid::new_v4().simple().to_string();
    let (dir, _lock) = initialize_run(root, &run)?;
    let started = Instant::now();
    let result = tokio::select! {
        result=services::bootstrap(&run,profile,slots,&dir)=>result,
        signal=signals.receive()=>{
            eprintln!("AURA_TEST_ENV {}",serde_json::json!({"event":"BOOTSTRAP_INTERRUPTED","run_id":run,
                "elapsed_ms":started.elapsed().as_millis(),"signal":signal}));
            cleanup(&run,&dir).await?;
            return Ok(128+signal);
        }
    };
    let result = match result {
        Ok(config) => run_child(&config, &dir, command, &mut signals, started).await,
        Err(error) => {
            eprintln!(
                "AURA_TEST_ENV {}",
                serde_json::json!({"event":"BOOTSTRAP_FAILED","run_id":run,
                "elapsed_ms":started.elapsed().as_millis()})
            );
            Err(error)
        }
    };
    let cleaned = cleanup(&run, &dir).await;
    match (result, cleaned) {
        (Ok(code), Ok(())) => Ok(code),
        (Ok(0), Err(error)) => Err(error),
        (Ok(code), Err(error)) => {
            eprintln!("owned cleanup failed: {error}");
            Ok(code)
        }
        (Err(error), cleanup) => {
            if let Err(cleanup) = cleanup {
                eprintln!("owned cleanup also failed: {cleanup}");
            }
            Err(error)
        }
    }
}

fn publish_environment(config: &ManagedEnvironment, dir: &Path) -> Result<PathBuf> {
    let path = dir.join("environment.json");
    std::fs::write(
        dir.join("environment.json.tmp"),
        serde_json::to_vec_pretty(config)?,
    )?;
    std::fs::rename(dir.join("environment.json.tmp"), &path)?;
    std::fs::write(dir.join("empty-aws-config"), [])?;
    Ok(path)
}

async fn run_child(
    config: &ManagedEnvironment,
    dir: &Path,
    command: &[std::ffi::OsString],
    signals: &mut process::Signals,
    started: Instant,
) -> Result<i32> {
    let path = publish_environment(config, dir)?;
    let count = usize::from(config.postgres.is_some())
        + usize::from(config.aws_endpoint.is_some())
        + usize::from(config.opensearch_endpoint.is_some());
    let generations = docker::generations(config.run_id(), count).await?;
    let mut child = process::spawn(command, config, &path)?;
    let group = child
        .id()
        .ok_or_else(|| invalid("supervised command has no process group"))?;
    if let Err(error) = std::fs::write(dir.join("child-group"), group.to_string()) {
        process::terminate(&mut child, group, libc::SIGTERM).await?;
        return Err(context("cannot persist supervised child ownership", error));
    }
    eprintln!(
        "AURA_TEST_ENV {}",
        serde_json::json!({"event":"READY","run_id":config.run_id(),
        "config":path,"slots":config.slots(),"bootstrap_ms":started.elapsed().as_millis()})
    );
    let code = tokio::select! {
        status=child.wait()=>process::exit_code(status?),
        signal=signals.receive()=>{
            process::terminate(&mut child,group,signal).await?;
            return Ok(128+signal);
        }
        result=docker::monitor(config.run_id(),generations)=>{
            process::terminate(&mut child,group,libc::SIGTERM).await?;
            return Err(context("supervised service health/restart boundary failed",
                result.err().unwrap_or_else(||invalid("service monitor stopped"))));
        }
    };
    process::terminate(&mut child, group, libc::SIGTERM).await?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_protect_initialization_before_the_run_lock_exists() {
        let root = default_state_dir().with_file_name(format!(
            "aura-test-env-initialization-{}",
            uuid::Uuid::new_v4().simple()
        ));
        directory(&root).unwrap();
        let initialization = lock_metadata(&root).unwrap();
        let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        directory(&pending).unwrap();
        let gc_root = root.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let collector = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(gc(&gc_root))
                .unwrap();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(pending.exists());
        let held = lock(&pending).unwrap().unwrap();
        drop(initialization);
        collector.join().unwrap();
        assert!(pending.exists());
        drop(held);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(gc(&root))
            .unwrap();
        assert!(!pending.exists());
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn should_skip_locked_initialization_and_collect_it_after_owner_exit() {
        let root = default_state_dir().with_file_name(format!(
            "aura-test-env-metadata-{}",
            uuid::Uuid::new_v4().simple()
        ));
        directory(&root).unwrap();
        let pending = root.join(format!(".pending-{}", uuid::Uuid::new_v4().simple()));
        directory(&pending).unwrap();
        let held = lock(&pending).unwrap().unwrap();
        gc(&root).await.unwrap();
        assert!(pending.exists());
        drop(held);
        gc(&root).await.unwrap();
        assert!(!pending.exists());
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn should_reject_state_directories_accessible_to_other_users() {
        let path = default_state_dir().with_file_name(format!(
            "aura-test-env-permissions-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(directory(&path).is_err());
        std::fs::remove_dir(path).unwrap();
    }
}
