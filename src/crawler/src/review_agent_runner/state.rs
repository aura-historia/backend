//! What the runner keeps between runs in its state directory, besides its process ledger: the
//! pause a usage limit set, each review's cooldown after a pass that ended without a decision,
//! and when the last daily digest was written (`review-agent-runner-state.json`); and the lock
//! that lets one runner work at a time (`review-agent-runner.lock`). A state file that cannot be
//! read fails the run instead of forgetting a pause or a cooldown.

use super::envelope::UsageLimitKind;
use crate::CrawlerReviewId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

pub const STATE_FILE: &str = "review-agent-runner-state.json";
pub const LOCK_FILE: &str = "review-agent-runner.lock";

/// The runner's pause after a usage limit of the CLI's login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
    pub limit: UsageLimitKind,
    /// Whether `until` is the reset time the CLI reported, not a guess.
    pub reported_reset: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerState {
    #[serde(default)]
    pub paused: Option<Pause>,
    /// Until when each review waits before its next pass.
    #[serde(default)]
    pub cooldowns: BTreeMap<CrawlerReviewId, Cooldown>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_digest: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cooldown {
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
}

impl RunnerState {
    /// The pause in force at `now`, if any.
    pub fn pause_at(&self, now: OffsetDateTime) -> Option<Pause> {
        self.paused.filter(|pause| pause.until > now)
    }

    /// Each review's cooldown end, for selecting the reviews of a pass.
    pub fn cooldown_ends(&self) -> BTreeMap<CrawlerReviewId, OffsetDateTime> {
        self.cooldowns
            .iter()
            .map(|(review_id, cooldown)| (*review_id, cooldown.until))
            .collect()
    }

    /// Forgets pauses and cooldowns that ended before `now`.
    pub fn expire(&mut self, now: OffsetDateTime) {
        self.paused = self.pause_at(now);
        self.cooldowns.retain(|_, cooldown| cooldown.until > now);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("cannot read or write the runner's state {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the runner's state {} is not the runner's state; fix or remove it", path.display())]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Reads the state in `state_dir`; a missing file is a fresh state.
pub fn load_state(state_dir: &Path) -> Result<RunnerState, StateError> {
    let path = state_dir.join(STATE_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|source| StateError::Malformed { path, source })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RunnerState::default()),
        Err(source) => Err(StateError::Io { path, source }),
    }
}

/// Writes `state` to `state_dir` under a temporary name and renames it into place, so a reader
/// never sees a partial state.
pub fn save_state(state_dir: &Path, state: &RunnerState) -> Result<(), StateError> {
    let path = state_dir.join(STATE_FILE);
    let io_error = |source| StateError::Io {
        path: path.clone(),
        source,
    };
    let bytes = serde_json::to_vec_pretty(state).map_err(|source| StateError::Malformed {
        path: path.clone(),
        source,
    })?;
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary).map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    std::fs::rename(&temporary, &path).map_err(io_error)
}

/// The lock one runner holds for its whole run; dropping it releases the lock.
#[derive(Debug)]
pub struct RunnerLock {
    _file: File,
}

impl RunnerLock {
    /// Takes the lock in `state_dir`, or `None` when another runner holds it.
    pub fn acquire(state_dir: &Path) -> Result<Option<Self>, StateError> {
        let path = state_dir.join(LOCK_FILE);
        let io_error = |source| StateError::Io {
            path: path.clone(),
            source,
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_error)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(io_error(source)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_agent_runner::ledger::tests::StateDirectory;
    use time::Duration;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-10-28 12:00 UTC);

    #[test]
    fn should_keep_pause_cooldowns_and_the_last_digest_across_runs() {
        let directory = StateDirectory::new();
        let review_id = CrawlerReviewId::new();
        let state = RunnerState {
            paused: Some(Pause {
                until: NOW + Duration::hours(2),
                limit: UsageLimitKind::Weekly,
                reported_reset: true,
            }),
            cooldowns: BTreeMap::from([(
                review_id,
                Cooldown {
                    until: NOW + Duration::minutes(30),
                },
            )]),
            last_digest: Some(NOW - Duration::hours(3)),
        };

        save_state(directory.path(), &state).unwrap_or_else(|e| panic!("{e}"));
        let mut read = load_state(directory.path()).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(read, state);
        assert_eq!(
            read.pause_at(NOW).map(|pause| pause.limit),
            Some(UsageLimitKind::Weekly)
        );
        read.expire(NOW + Duration::hours(3));
        assert_eq!(read.paused, None);
        assert!(read.cooldowns.is_empty());
        assert_eq!(read.last_digest, state.last_digest);
    }

    #[test]
    fn should_start_fresh_without_a_state_and_refuse_a_state_it_cannot_read() {
        let directory = StateDirectory::new();
        assert_eq!(
            load_state(directory.path()).ok(),
            Some(RunnerState::default())
        );

        std::fs::write(directory.path().join(STATE_FILE), "{\"paused\": 1}").unwrap_or_default();

        assert!(matches!(
            load_state(directory.path()),
            Err(StateError::Malformed { .. })
        ));
    }

    #[test]
    fn should_let_one_runner_hold_the_lock_at_a_time() {
        let directory = StateDirectory::new();

        let first = RunnerLock::acquire(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        let second = RunnerLock::acquire(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        drop(first);
        let third = RunnerLock::acquire(directory.path()).unwrap_or_else(|e| panic!("{e}"));

        assert!(second.is_none());
        assert!(third.is_some());
    }
}
