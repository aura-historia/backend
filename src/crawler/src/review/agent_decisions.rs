//! What the review agents may decide on their own: whether the reviewer agent's approvals and
//! rejections are taken at all, which only `agent` authoring mode turns on, how many of them per
//! hour, how recent the live matrix or the sample labels an approval rests on must be, and how
//! many sample URLs of each label a URL-pattern approval needs. The operator contract is in
//! `docs/crawler/operations.md` (review agents); the checks a guarded approval passes are in
//! [`crate::review::approval_guard`].

use crate::review::access::ReviewServerConfigError;
use std::num::NonZeroU32;
use std::time::Duration;

pub const AGENT_MAX_DECISIONS_PER_HOUR_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR";
pub const AGENT_MATRIX_MAX_AGE_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES";
pub const AGENT_MIN_LABELLED_URLS_ENV: &str = "CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS";

const DEFAULT_MAX_DECISIONS_PER_HOUR: NonZeroU32 = NonZeroU32::new(20).unwrap();
const DEFAULT_MATRIX_MAX_AGE_MINUTES: NonZeroU32 = NonZeroU32::new(30).unwrap();
const DEFAULT_MIN_LABELLED_URLS: NonZeroU32 = NonZeroU32::new(3).unwrap();

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

/// The limits of the review agents' own decisions, approvals and rejections alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDecisionPolicy {
    pub approvals: AgentApprovals,
    /// The agent's approvals plus rejections taken within any rolling hour.
    pub max_decisions_per_hour: NonZeroU32,
    /// How long ago the review pages of the live schema matrix an agent approval rests on may
    /// have been fetched, or the sample labels of a URL pattern it rests on set.
    pub max_matrix_age: Duration,
    /// How many URLs an agent approval of a URL pattern needs labelled `PRODUCT`, and how many
    /// labelled `NOT_PRODUCT`.
    pub min_labelled_urls: NonZeroU32,
}

impl Default for AgentDecisionPolicy {
    /// Agent decisions off: the agent decides nothing until an operator opts in.
    fn default() -> Self {
        Self {
            approvals: AgentApprovals::Off,
            max_decisions_per_hour: DEFAULT_MAX_DECISIONS_PER_HOUR,
            max_matrix_age: minutes(DEFAULT_MATRIX_MAX_AGE_MINUTES),
            min_labelled_urls: DEFAULT_MIN_LABELLED_URLS,
        }
    }
}

impl AgentDecisionPolicy {
    /// Reads the limits of agent decisions, which are off until the review console of `agent`
    /// authoring mode turns them on. An empty or whitespace-only value counts as unset; a value
    /// that is set but not accepted fails startup instead of silently changing what the agents
    /// may decide.
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let defaults = Self::default();
        let max_decisions_per_hour = positive_number(AGENT_MAX_DECISIONS_PER_HOUR_ENV)?
            .unwrap_or(defaults.max_decisions_per_hour);
        let max_matrix_age = positive_number(AGENT_MATRIX_MAX_AGE_MINUTES_ENV)?
            .map_or(defaults.max_matrix_age, minutes);
        let min_labelled_urls =
            positive_number(AGENT_MIN_LABELLED_URLS_ENV)?.unwrap_or(defaults.min_labelled_urls);
        Ok(Self {
            approvals: defaults.approvals,
            max_decisions_per_hour,
            max_matrix_age,
            min_labelled_urls,
        })
    }
}

fn minutes(count: NonZeroU32) -> Duration {
    Duration::from_secs(u64::from(count.get()) * 60)
}

/// The trimmed value of `name`, `None` when it is unset, empty or whitespace-only. A value that
/// is not valid Unicode is returned as `Err` to be refused like any other value not accepted.
fn setting(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(raw.trim().to_owned())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(raw)) => Err(raw.to_string_lossy().into_owned()),
    }
}

fn positive_number(name: &'static str) -> Result<Option<NonZeroU32>, ReviewServerConfigError> {
    setting(name)
        .and_then(|raw| {
            raw.map(|raw| raw.parse::<NonZeroU32>().map_err(|_| raw))
                .transpose()
        })
        .map_err(|value| ReviewServerConfigError::InvalidPositiveNumber { name, value })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: [&str; 3] = [
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
        AGENT_MIN_LABELLED_URLS_ENV,
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
            ][..],
        ] {
            let policy = policy_from(env).unwrap_or_else(|error| panic!("{env:?}: {error}"));

            assert_eq!(policy, AgentDecisionPolicy::default(), "{env:?}");
            assert_eq!(policy.approvals, AgentApprovals::Off);
            assert_eq!(policy.max_decisions_per_hour.get(), 20);
            assert_eq!(policy.max_matrix_age, Duration::from_secs(30 * 60));
            assert_eq!(policy.min_labelled_urls.get(), 3);
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
        ])
        .unwrap_or_else(|error| panic!("agent decision limits: {error}"));

        assert_eq!(
            policy,
            AgentDecisionPolicy {
                approvals: AgentApprovals::Off,
                max_decisions_per_hour: NonZeroU32::new(5).unwrap(),
                max_matrix_age: Duration::from_secs(10 * 60),
                min_labelled_urls: NonZeroU32::new(4).unwrap(),
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
}
