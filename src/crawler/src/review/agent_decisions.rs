//! What the review agents may decide on their own: whether the reviewer agent's approvals and
//! rejections are taken at all, which only `agent` authoring mode turns on, how many of them per
//! hour, how recent the live matrix a product-schema approval rests on must be, how many sample
//! URLs of each label a URL-pattern approval needs, and how many needs-repair rounds the agents
//! give one review before it waits for a human. The operator contract is in
//! `docs/crawler/operations.md` (review agents); the checks a guarded approval passes are in
//! [`crate::review::approval_guard`].

use crate::review::access::{ReviewServerConfigError, positive_number};
use crate::review::model::STATUS_PENDING_REVIEW;
use std::num::NonZeroU32;
use std::time::Duration;

pub const AGENT_MAX_DECISIONS_PER_HOUR_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR";
pub const AGENT_MATRIX_MAX_AGE_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES";
pub const AGENT_MIN_LABELLED_URLS_ENV: &str = "CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS";
pub const AGENT_MAX_REPAIR_ROUNDS_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS";

const DEFAULT_MAX_DECISIONS_PER_HOUR: NonZeroU32 = NonZeroU32::new(20).unwrap();
const DEFAULT_MATRIX_MAX_AGE_MINUTES: NonZeroU32 = NonZeroU32::new(30).unwrap();
pub const DEFAULT_MIN_LABELLED_URLS: NonZeroU32 = NonZeroU32::new(3).unwrap();
const DEFAULT_MAX_REPAIR_ROUNDS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// Whether the review agents' approvals and rejections are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentApprovals {
    /// Refused: a human decides every review.
    #[default]
    Off,
    /// Taken when the server's deterministic checks pass: the review console of `agent` authoring
    /// mode.
    Guarded,
}

impl AgentApprovals {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Guarded => "guarded",
        }
    }
}

/// The limits of the review agents' own decisions, approvals and rejections alike: the
/// `CRAWLER_REVIEW_AGENT_*` settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentLimits {
    /// The agent's approvals plus rejections taken within any rolling hour.
    pub max_decisions_per_hour: NonZeroU32,
    /// How many minutes ago the review pages of the live schema matrix an agent approval rests on
    /// may have been fetched. A URL pattern's sample labels do not expire: they belong to the
    /// candidate version an approval names.
    pub matrix_max_age_minutes: NonZeroU32,
    /// How many URLs an agent approval of a URL pattern needs labelled `PRODUCT`, and how many
    /// labelled `NOT_PRODUCT`.
    pub min_labelled_urls: NonZeroU32,
    /// How many needs-repair verdicts the agents give a pending review, counted since a human last
    /// changed or judged it, before it waits for a human and the agents may change or decide
    /// nothing on it.
    pub max_repair_rounds: NonZeroU32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_decisions_per_hour: DEFAULT_MAX_DECISIONS_PER_HOUR,
            matrix_max_age_minutes: DEFAULT_MATRIX_MAX_AGE_MINUTES,
            min_labelled_urls: DEFAULT_MIN_LABELLED_URLS,
            max_repair_rounds: DEFAULT_MAX_REPAIR_ROUNDS,
        }
    }
}

impl AgentLimits {
    /// Reads the limits of agent decisions. An empty or whitespace-only value counts as unset; a
    /// value that is set but not accepted fails startup instead of silently changing what the
    /// agents may decide.
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let defaults = Self::default();
        let read = |name, default| positive_number(name).map(|value| value.unwrap_or(default));
        Ok(Self {
            max_decisions_per_hour: read(
                AGENT_MAX_DECISIONS_PER_HOUR_ENV,
                defaults.max_decisions_per_hour,
            )?,
            matrix_max_age_minutes: read(
                AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
                defaults.matrix_max_age_minutes,
            )?,
            min_labelled_urls: read(AGENT_MIN_LABELLED_URLS_ENV, defaults.min_labelled_urls)?,
            max_repair_rounds: read(AGENT_MAX_REPAIR_ROUNDS_ENV, defaults.max_repair_rounds)?,
        })
    }

    /// How long ago the review pages of the live schema matrix an agent approval rests on may have
    /// been fetched.
    pub fn max_matrix_age(&self) -> Duration {
        Duration::from_secs(u64::from(self.matrix_max_age_minutes.get()) * 60)
    }
}

/// What the review agents may decide on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDecisionPolicy {
    pub approvals: AgentApprovals,
    pub limits: AgentLimits,
}

impl Default for AgentDecisionPolicy {
    /// Agent decisions off: the agent decides nothing until an operator opts in.
    fn default() -> Self {
        Self {
            approvals: AgentApprovals::Off,
            limits: AgentLimits::default(),
        }
    }
}

impl AgentDecisionPolicy {
    /// Reads the limits of agent decisions, which are off until the review console of `agent`
    /// authoring mode turns them on.
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        Ok(Self {
            approvals: AgentApprovals::Off,
            limits: AgentLimits::from_env()?,
        })
    }

    /// Whether a review in `status` that the agents gave `agent_rounds` needs-repair verdicts
    /// since a human last changed or judged it waits for a human: it is pending and their
    /// back-and-forth on it reached the cap.
    pub fn waits_for_human(&self, status: &str, agent_rounds: u32) -> bool {
        status == STATUS_PENDING_REVIEW && agent_rounds >= self.limits.max_repair_rounds.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: [&str; 4] = [
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
        AGENT_MIN_LABELLED_URLS_ENV,
        AGENT_MAX_REPAIR_ROUNDS_ENV,
    ];

    // Tests that touch the process environment are serialized, so no test reads the
    // environment while another one changes it.
    fn policy_from(env: &[(&str, &str)]) -> Result<AgentDecisionPolicy, ReviewServerConfigError> {
        let set = |name: &str, value: Option<&str>| match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        };
        for name in SETTINGS {
            set(name, None);
        }
        for (name, value) in env {
            set(name, Some(value));
        }
        let policy = AgentDecisionPolicy::from_env();
        for name in SETTINGS {
            set(name, None);
        }
        policy
    }

    #[test]
    #[serial_test::serial]
    fn should_turn_agent_decisions_off_with_twenty_per_hour_when_nothing_is_set() {
        for env in [
            &[][..],
            &[
                (AGENT_MAX_DECISIONS_PER_HOUR_ENV, " "),
                (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, ""),
                (AGENT_MIN_LABELLED_URLS_ENV, "\t"),
                (AGENT_MAX_REPAIR_ROUNDS_ENV, "  "),
            ][..],
        ] {
            let policy = policy_from(env).unwrap_or_else(|error| panic!("{env:?}: {error}"));

            assert_eq!(policy, AgentDecisionPolicy::default(), "{env:?}");
            assert_eq!(policy.approvals, AgentApprovals::Off);
            assert_eq!(policy.limits.max_decisions_per_hour.get(), 20);
            assert_eq!(policy.limits.max_matrix_age(), Duration::from_secs(30 * 60));
            assert_eq!(policy.limits.min_labelled_urls.get(), 3);
            assert_eq!(policy.limits.max_repair_rounds.get(), 3);
        }
    }

    /// The limits are read from the environment; whether agents decide at all is not.
    #[test]
    #[serial_test::serial]
    fn should_read_the_limits_of_agent_decisions_and_leave_them_off() {
        let policy = policy_from(&[
            (AGENT_MAX_DECISIONS_PER_HOUR_ENV, "5"),
            (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, "10"),
            (AGENT_MIN_LABELLED_URLS_ENV, " 4 "),
            (AGENT_MAX_REPAIR_ROUNDS_ENV, "2"),
        ])
        .unwrap_or_else(|error| panic!("agent decision limits: {error}"));

        assert_eq!(
            policy,
            AgentDecisionPolicy {
                approvals: AgentApprovals::Off,
                limits: AgentLimits {
                    max_decisions_per_hour: NonZeroU32::new(5).unwrap(),
                    matrix_max_age_minutes: NonZeroU32::new(10).unwrap(),
                    min_labelled_urls: NonZeroU32::new(4).unwrap(),
                    max_repair_rounds: NonZeroU32::new(2).unwrap(),
                },
            }
        );
    }

    /// Like `CRAWLER_AUTHORING_MODE`, a value that is set but not accepted fails startup with
    /// the variable, the value and what is accepted.
    #[rstest::rstest]
    #[case::zero_decisions(
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        "0",
        "invalid CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR value \"0\"; expected a whole number of at least 1"
    )]
    #[case::fraction_of_decisions(
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        "2.5",
        "invalid CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR value \"2.5\"; expected a whole number of at least 1"
    )]
    #[case::negative_age(
        AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
        "-1",
        "invalid CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES value \"-1\"; expected a whole number of at least 1"
    )]
    #[case::no_labelled_urls(
        AGENT_MIN_LABELLED_URLS_ENV,
        "0",
        "invalid CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS value \"0\"; expected a whole number of at least 1"
    )]
    #[case::no_repair_rounds(
        AGENT_MAX_REPAIR_ROUNDS_ENV,
        "0",
        "invalid CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS value \"0\"; expected a whole number of at least 1"
    )]
    #[case::repair_rounds_in_words(
        AGENT_MAX_REPAIR_ROUNDS_ENV,
        "three",
        "invalid CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS value \"three\"; expected a whole number of at least 1"
    )]
    #[serial_test::serial]
    fn should_refuse_to_start_with_a_value_that_is_not_accepted(
        #[case] name: &'static str,
        #[case] value: &'static str,
        #[case] message: &str,
    ) {
        let error = policy_from(&[(name, value)])
            .err()
            .unwrap_or_else(|| panic!("{name}={value} must fail startup"));

        assert_eq!(error.to_string(), message);
    }

    /// The verdict that reaches the cap is still taken; from then on the pending review waits for a
    /// human, and a decided review waits for nobody.
    #[test]
    fn should_hand_a_review_to_a_human_once_the_agents_reached_the_cap() {
        let policy = AgentDecisionPolicy {
            limits: AgentLimits {
                max_repair_rounds: NonZeroU32::new(3).unwrap(),
                ..AgentLimits::default()
            },
            ..AgentDecisionPolicy::default()
        };

        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, 0));
        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, 2));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, 3));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, 4));
        for decided in ["APPROVED", "REJECTED", "SUPERSEDED"] {
            assert!(!policy.waits_for_human(decided, 3), "{decided}");
        }
    }
}
