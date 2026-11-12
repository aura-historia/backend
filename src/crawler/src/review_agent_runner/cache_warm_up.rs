//! Warming the model's prompt cache before a pass starts processes in parallel. Every process of
//! one role on one artifact type opens with the same prefix: the system prompt, the tool
//! definitions and the role's prompt before the review is named, about 28,000 tokens. The prompt
//! cache serves that prefix to every later process at a fraction of the price of writing it, but
//! only once the first process wrote it: processes started together each write it themselves. So
//! the first process of a prefix starts at once and the others of that prefix wait until it had
//! time to write it (`docs/crawler/operations.md`, review agent runner).

use super::queue::ReviewArtifact;
use crate::review_mcp::AgentRole;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::time::Duration;
use tokio::time::Instant;

/// When the first process of each prefix of a pass started, and how long the others wait for it.
#[derive(Debug)]
pub(super) struct CacheWarmUp {
    warm_up: Duration,
    first_started: BTreeMap<(&'static str, &'static str), Instant>,
}

impl CacheWarmUp {
    /// Processes of a prefix wait `warm_up` from its first one's start, the time it has to send
    /// its first model request, whose answer writes the prefix to the cache.
    pub(super) fn new(warm_up: Duration) -> Self {
        Self {
            warm_up,
            first_started: BTreeMap::new(),
        }
    }

    /// How long a process of `role` on `artifact` that starts at `now` waits so the prefix's first
    /// process can write the cache: nothing for that first process itself and for every process
    /// once the first had the warm-up.
    pub(super) fn wait_before_start(
        &mut self,
        role: AgentRole,
        artifact: ReviewArtifact,
        now: Instant,
    ) -> Option<Duration> {
        match self.first_started.entry((role.as_str(), artifact.as_str())) {
            Entry::Vacant(first) => {
                first.insert(now);
                None
            }
            Entry::Occupied(first) => (*first.get() + self.warm_up)
                .checked_duration_since(now)
                .filter(|wait| !wait.is_zero()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARM_UP: Duration = Duration::from_secs(20);

    #[test]
    fn should_start_the_first_process_of_a_prefix_at_once_and_the_others_after_it_wrote_the_cache()
    {
        let mut warm_up = CacheWarmUp::new(WARM_UP);
        let start = Instant::now();
        let author = |warm_up: &mut CacheWarmUp, at: Duration| {
            warm_up.wait_before_start(AgentRole::Author, ReviewArtifact::ProductSchema, start + at)
        };

        assert_eq!(author(&mut warm_up, Duration::ZERO), None);
        assert_eq!(
            author(&mut warm_up, Duration::from_secs(1)),
            Some(WARM_UP - Duration::from_secs(1))
        );
        assert_eq!(author(&mut warm_up, WARM_UP), None);
        assert_eq!(author(&mut warm_up, WARM_UP * 3), None);
    }

    #[test]
    fn should_warm_each_role_and_artifact_type_on_its_own() {
        let mut warm_up = CacheWarmUp::new(WARM_UP);
        let now = Instant::now();

        assert_eq!(
            warm_up.wait_before_start(AgentRole::Author, ReviewArtifact::ProductSchema, now),
            None
        );
        assert_eq!(
            warm_up.wait_before_start(AgentRole::Reviewer, ReviewArtifact::ProductSchema, now),
            None
        );
        assert_eq!(
            warm_up.wait_before_start(AgentRole::Author, ReviewArtifact::UrlPattern, now),
            None
        );
    }
}
