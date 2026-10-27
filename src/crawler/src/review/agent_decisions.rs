//! What the review agent may decide on its own: whether its approvals and rejections are taken at
//! all, how many of them per hour, and how recent the live evaluation an approval rests on must
//! be. The operator contract is in `docs/crawler/operations.md` (review service); the checks a
//! guarded approval passes are in [`crate::review::approval_guard`].

use crate::review::access::ReviewServerConfigError;
use std::num::NonZeroU32;
use std::time::Duration;

pub const AGENT_APPROVALS_ENV: &str = "CRAWLER_REVIEW_AGENT_APPROVALS";
pub const AGENT_MAX_DECISIONS_PER_HOUR_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR";
pub const AGENT_MATRIX_MAX_AGE_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES";

const DEFAULT_MAX_DECISIONS_PER_HOUR: NonZeroU32 = NonZeroU32::new(20).unwrap();
const DEFAULT_MATRIX_MAX_AGE_MINUTES: NonZeroU32 = NonZeroU32::new(30).unwrap();

/// Whether the review agent's approvals and rejections are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentApprovals {
    /// Refused: the agent stages candidates, pages and notes, and a human decides.
    #[default]
    Off,
    /// Taken when the server's deterministic checks pass.
    Guarded,
}

impl AgentApprovals {
    pub const ALL: [Self; 2] = [Self::Off, Self::Guarded];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Guarded => "guarded",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|approvals| approvals.as_str().eq_ignore_ascii_case(raw.trim()))
    }
}

/// The limits of the review agent's own decisions, approvals and rejections alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDecisionPolicy {
    pub approvals: AgentApprovals,
    /// The agent's approvals plus rejections taken within any rolling hour.
    pub max_decisions_per_hour: NonZeroU32,
    /// How long ago the review pages of the live schema matrix an agent approval rests on may
    /// have been fetched.
    pub max_matrix_age: Duration,
}

impl Default for AgentDecisionPolicy {
    /// Agent decisions off: the agent decides nothing until an operator opts in.
    fn default() -> Self {
        Self {
            approvals: AgentApprovals::Off,
            max_decisions_per_hour: DEFAULT_MAX_DECISIONS_PER_HOUR,
            max_matrix_age: minutes(DEFAULT_MATRIX_MAX_AGE_MINUTES),
        }
    }
}

impl AgentDecisionPolicy {
    /// Reads the agent decision settings, where an empty or whitespace-only value counts as
    /// unset. A value that is set but not accepted fails startup instead of silently changing
    /// what the agent may decide.
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let defaults = Self::default();
        let approvals = setting(AGENT_APPROVALS_ENV)
            .and_then(|raw| {
                raw.map(|raw| AgentApprovals::parse(&raw).ok_or(raw))
                    .transpose()
            })
            .map_err(|value| ReviewServerConfigError::InvalidValue {
                name: AGENT_APPROVALS_ENV,
                value,
                accepted: AgentApprovals::ALL.map(AgentApprovals::as_str).join(", "),
            })?
            .unwrap_or(defaults.approvals);
        let max_decisions_per_hour = positive_number(AGENT_MAX_DECISIONS_PER_HOUR_ENV)?
            .unwrap_or(defaults.max_decisions_per_hour);
        let max_matrix_age = positive_number(AGENT_MATRIX_MAX_AGE_MINUTES_ENV)?
            .map_or(defaults.max_matrix_age, minutes);
        Ok(Self {
            approvals,
            max_decisions_per_hour,
            max_matrix_age,
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
        AGENT_APPROVALS_ENV,
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
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
                (AGENT_APPROVALS_ENV, ""),
                (AGENT_MAX_DECISIONS_PER_HOUR_ENV, " "),
                (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, ""),
            ][..],
        ] {
            let policy = policy_from(env).unwrap_or_else(|error| panic!("{env:?}: {error}"));

            assert_eq!(policy, AgentDecisionPolicy::default(), "{env:?}");
            assert_eq!(policy.approvals, AgentApprovals::Off);
            assert_eq!(policy.max_decisions_per_hour.get(), 20);
            assert_eq!(policy.max_matrix_age, Duration::from_secs(30 * 60));
        }
    }

    #[test]
    #[serial_test::serial]
    fn should_read_guarded_approvals_and_their_limits() {
        let policy = policy_from(&[
            (AGENT_APPROVALS_ENV, " Guarded "),
            (AGENT_MAX_DECISIONS_PER_HOUR_ENV, "5"),
            (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, "10"),
        ])
        .unwrap_or_else(|error| panic!("guarded settings: {error}"));

        assert_eq!(
            policy,
            AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                max_decisions_per_hour: NonZeroU32::new(5).unwrap(),
                max_matrix_age: Duration::from_secs(10 * 60),
            }
        );
    }

    /// Like `CRAWLER_AUTHORING_MODE`, a value that is set but not accepted fails startup with
    /// the variable, the value and what is accepted.
    #[rstest::rstest]
    #[case::unknown_mode(
        AGENT_APPROVALS_ENV,
        "on",
        "invalid CRAWLER_REVIEW_AGENT_APPROVALS value \"on\"; expected one of: off, guarded"
    )]
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
