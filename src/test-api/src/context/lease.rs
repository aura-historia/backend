use super::{Isolation, Result, failure};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const WAIT: Duration = Duration::from_secs(120);
static ALLOCATION: OnceLock<Allocation> = OnceLock::new();
static INITIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());

extern "C" fn cleanup_local_directory() {
    if crate::environment::managed_environment().is_none()
        && let Some(allocation) = ALLOCATION.get()
    {
        let _ = std::fs::remove_dir_all(&allocation.directory);
    }
}

#[derive(Debug)]
pub(super) struct Runner {
    pub invocation: String,
    pub slot: Option<u16>,
}

impl Runner {
    pub fn parse(
        marker: Option<&str>,
        run: Option<&str>,
        slot: Option<&str>,
        mode: Option<&str>,
        slots: u16,
    ) -> Result<Self> {
        match (marker, run, slot, mode) {
            (None, None, None, None) => Ok(Self {
                invocation: uuid::Uuid::new_v4().simple().to_string(),
                slot: None,
            }),
            (Some("1"), Some(run), Some(slot), Some("process-per-test")) => {
                let run = uuid::Uuid::parse_str(run)?;
                let slot: u16 = slot.parse()?;
                if run.is_nil() || slot >= slots {
                    return Err(failure("invalid Nextest invocation or global slot"));
                }
                Ok(Self {
                    invocation: run.simple().to_string(),
                    slot: Some(slot),
                })
            }
            _ => Err(failure(
                "incomplete Nextest contract: require NEXTEST=1, RUN_ID, TEST_GLOBAL_SLOT and process-per-test mode",
            )),
        }
    }

    pub fn environment(slots: u16) -> Result<Self> {
        let values = [
            "NEXTEST",
            "NEXTEST_RUN_ID",
            "NEXTEST_TEST_GLOBAL_SLOT",
            "NEXTEST_EXECUTION_MODE",
        ]
        .map(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error),
        });
        let [marker, run, slot, mode] = values;
        let values = [marker?, run?, slot?, mode?];
        Self::parse(
            values[0].as_deref(),
            values[1].as_deref(),
            values[2].as_deref(),
            values[3].as_deref(),
            slots,
        )
    }
}

pub(super) struct Allocation {
    pub run: String,
    pub directory: PathBuf,
    pub slots: u16,
    semaphore: Arc<Semaphore>,
}

impl Allocation {
    pub async fn current() -> Result<&'static Self> {
        if let Some(value) = ALLOCATION.get() {
            return Ok(value);
        }
        tokio::task::spawn_blocking(Self::initialize).await?
    }

    fn initialize() -> Result<&'static Self> {
        let _initialization = INITIALIZE.lock().expect("allocation initialization lock");
        if let Some(value) = ALLOCATION.get() {
            return Ok(value);
        }
        let (run, directory, slots) = if let Some(env) = crate::environment::managed_environment() {
            (
                env.run_id().to_owned(),
                crate::environment::managed_directory()
                    .expect("validated managed directory")
                    .to_owned(),
                env.slots(),
            )
        } else {
            let run = uuid::Uuid::new_v4().simple().to_string();
            // Unmanaged containers belong to this process. Their leases cannot share another process's service.
            let directory =
                std::env::temp_dir().join(format!("aura-test-leases-{}-{run}", std::process::id()));
            (run, directory, 1)
        };
        private_directory(&directory)?;
        let allocation = Self {
            run,
            directory,
            slots,
            semaphore: Arc::new(Semaphore::new(usize::from(slots))),
        };
        let _ = ALLOCATION.set(allocation);
        if crate::environment::managed_environment().is_none() {
            unsafe { libc::atexit(cleanup_local_directory) };
        }
        Ok(ALLOCATION.get().expect("allocation initialized"))
    }

    pub async fn reserve(&self, runner: Runner, isolation: Isolation) -> Result<Reservation> {
        let mut permit =
            tokio::time::timeout(WAIT, self.semaphore.clone().acquire_owned()).await??;
        let started = Instant::now();
        loop {
            let directory = self.directory.clone();
            let slots = self.slots;
            let requested = runner.slot;
            let invocation = runner.invocation.clone();
            // Transfer the permit into the operation: cancellation cannot release it while
            // a lock attempt is still running on the blocking pool.
            let attempt = tokio::task::spawn_blocking(move || {
                let gate = lock_file(&directory.join("tests.lock"))?;
                if let Some(gate) = FileLock::attempt(gate, isolation)? {
                    for slot in requested.map_or_else(|| (0..slots).collect(), |slot| vec![slot]) {
                        if slot >= slots {
                            return Err(failure("requested slot exceeds supervisor capacity"));
                        }
                        let file = lock_file(&directory.join(format!("slot-{slot}.lock")))?;
                        if let Some(file) = FileLock::attempt(file, Isolation::Exclusive)? {
                            return Ok(ReservationAttempt::Reserved(Reservation {
                                slot,
                                invocation,
                                directory,
                                _gate: gate,
                                _slot: file,
                                _permit: permit,
                            }));
                        }
                    }
                }
                Ok(ReservationAttempt::Busy(permit))
            })
            .await??;
            match attempt {
                ReservationAttempt::Reserved(reservation) => return Ok(reservation),
                ReservationAttempt::Busy(returned) => permit = returned,
            }
            if started.elapsed() >= WAIT {
                return Err(failure(
                    "timed out reserving a test slot; another invocation still owns it",
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

enum ReservationAttempt {
    Busy(OwnedSemaphorePermit),
    Reserved(Reservation),
}

/// Short metadata/lock attempts run off the async workers and keep the file owned
/// by the blocking operation until it completes, including after cancellation.
#[cfg(feature = "postgres")]
pub(super) async fn exclusive_file(path: PathBuf) -> Result<FileLock> {
    let started = Instant::now();
    loop {
        let attempt_path = path.clone();
        if let Some(file) = tokio::task::spawn_blocking(move || {
            let file = lock_file(&attempt_path)?;
            FileLock::attempt(file, Isolation::Exclusive)
        })
        .await??
        {
            return Ok(file);
        }
        if started.elapsed() >= WAIT {
            return Err(failure("timed out locking Postgres template preparation"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(super) struct Reservation {
    pub slot: u16,
    pub invocation: String,
    pub directory: PathBuf,
    _gate: FileLock,
    _slot: FileLock,
    _permit: OwnedSemaphorePermit,
}

/// Explicit unlock matters even with close-on-exec: a concurrent fork can briefly
/// inherit the same open file description before exec closes its duplicate.
pub(super) struct FileLock(File);

impl FileLock {
    fn attempt(file: File, isolation: Isolation) -> Result<Option<Self>> {
        let result = match isolation {
            Isolation::Isolated => file.try_lock_shared(),
            Isolation::Exclusive => file.try_lock(),
        };
        Ok(available(result)?.then(|| Self(file)))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            eprintln!("failed to unlock owned test reservation: {error}");
        }
    }
}

pub(super) fn private_directory(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(failure(
            "test lease directory must be private and owned by the current user",
        ));
    }
    Ok(())
}

pub(super) fn lock_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(failure("test lock must be an owned private regular file"));
    }
    Ok(file)
}

fn available(result: std::result::Result<(), std::fs::TryLockError>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_reject_partial_nextest_contract_and_invalid_slots() {
        let run = "d4c0e63c-fb63-47b3-ab89-72502d963d1a";
        assert!(Runner::parse(None, None, None, None, 2).is_ok());
        assert_eq!(
            Some(1),
            Runner::parse(Some("1"), Some(run), Some("1"), Some("process-per-test"), 2)
                .unwrap()
                .slot
        );
        for slot in ["-1", "2", "65536", "one", ""] {
            assert!(
                Runner::parse(
                    Some("1"),
                    Some(run),
                    Some(slot),
                    Some("process-per-test"),
                    2
                )
                .is_err()
            );
        }
        assert!(Runner::parse(None, Some(run), None, None, 2).is_err());
        assert!(
            Runner::parse(
                Some("1"),
                Some("foreign"),
                Some("0"),
                Some("process-per-test"),
                2
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    fn allocation(directory: PathBuf) -> Allocation {
        Allocation {
            run: "0".repeat(32),
            directory,
            slots: 2,
            semaphore: Arc::new(Semaphore::new(2)),
        }
    }
    fn runner(slot: u16) -> Runner {
        Runner {
            invocation: uuid::Uuid::new_v4().simple().to_string(),
            slot: Some(slot),
        }
    }
    #[tokio::test]
    async fn reservation_child() {
        let Some(directory) = std::env::var_os("AURA_LEASE_CHILD_DIRECTORY") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let mode = std::env::var("AURA_LEASE_CHILD_MODE").unwrap();
        let isolation = if mode == "exclusive" {
            Isolation::Exclusive
        } else {
            Isolation::Isolated
        };
        let allocation = allocation(directory.clone());
        tokio::fs::write(directory.join("child-attempting"), [])
            .await
            .unwrap();
        let _lease = allocation.reserve(runner(0), isolation).await.unwrap();
        tokio::fs::write(directory.join("child-ready"), [])
            .await
            .unwrap();
        while !tokio::fs::try_exists(directory.join("child-release"))
            .await
            .unwrap()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn contention(mode: &str) {
        let directory =
            std::env::temp_dir().join(format!("aura-lease-unit-{}", uuid::Uuid::new_v4().simple()));
        let create_directory = directory.clone();
        tokio::task::spawn_blocking(move || private_directory(&create_directory))
            .await
            .unwrap()
            .unwrap();
        let allocation = allocation(directory.clone());
        let first = allocation
            .reserve(
                runner(if mode == "exclusive" { 1 } else { 0 }),
                Isolation::Isolated,
            )
            .await
            .unwrap();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "context::lease::process_tests::reservation_child",
                "--nocapture",
            ])
            .env("AURA_LEASE_CHILD_DIRECTORY", &directory)
            .env("AURA_LEASE_CHILD_MODE", mode)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !tokio::fs::try_exists(directory.join("child-attempting"))
            .await
            .unwrap()
        {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !tokio::fs::try_exists(directory.join("child-ready"))
                .await
                .unwrap(),
            "another process must not overlap the live reservation"
        );
        drop(first);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !tokio::fs::try_exists(directory.join("child-ready"))
            .await
            .unwrap()
        {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::fs::write(directory.join("child-release"), [])
            .await
            .unwrap();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
    #[tokio::test]
    async fn should_exclude_a_different_process_even_when_it_requests_a_different_slot() {
        contention("exclusive").await;
    }
    #[tokio::test]
    async fn should_serialize_same_slot_across_different_runner_invocations() {
        contention("slot").await;
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[tokio::test]
    async fn should_allocate_available_slots_and_bound_explicit_waiters_without_leaking_permits() {
        let directory = std::env::temp_dir().join(format!(
            "aura-explicit-unit-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let create_directory = directory.clone();
        tokio::task::spawn_blocking(move || private_directory(&create_directory))
            .await
            .unwrap()
            .unwrap();
        let allocation = Allocation {
            run: "0".repeat(32),
            directory: directory.clone(),
            slots: 2,
            semaphore: Arc::new(Semaphore::new(2)),
        };
        let runner = || Runner::parse(None, None, None, None, 2).unwrap();
        let first = allocation
            .reserve(runner(), Isolation::Isolated)
            .await
            .unwrap();
        let second = allocation
            .reserve(runner(), Isolation::Isolated)
            .await
            .unwrap();
        assert_ne!(first.slot, second.slot);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(25),
                allocation.reserve(runner(), Isolation::Isolated)
            )
            .await
            .is_err()
        );
        let free = first.slot;
        drop(first);
        let third = allocation
            .reserve(runner(), Isolation::Isolated)
            .await
            .unwrap();
        assert_eq!(free, third.slot);
        drop(third);
        drop(second);
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}

#[cfg(test)]
mod lock_ownership_tests {
    use super::*;

    #[test]
    fn should_unlock_an_inherited_file_description_only_after_the_last_owner_finishes() {
        let directory =
            std::env::temp_dir().join(format!("aura-lock-owner-{}", uuid::Uuid::new_v4().simple()));
        private_directory(&directory).unwrap();
        let path = directory.join("owned.lock");
        let guard = Arc::new(
            FileLock::attempt(lock_file(&path).unwrap(), Isolation::Exclusive)
                .unwrap()
                .unwrap(),
        );
        // dup shares the open file description just as a forked child does before exec.
        let inherited = guard.0.try_clone().unwrap();
        let pending_operation = guard.clone();
        drop(guard);
        assert!(
            FileLock::attempt(lock_file(&path).unwrap(), Isolation::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(pending_operation);
        let next = FileLock::attempt(lock_file(&path).unwrap(), Isolation::Exclusive)
            .unwrap()
            .expect("explicit unlock must release the lock despite the inherited descriptor");
        drop(next);
        drop(inherited);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
