//! What the review agents may decide on their own in `agent` authoring mode: how many approvals
//! and rejections per hour, how recent the live matrix a product-schema approval rests on must
//! be, how many sample URLs of each label a URL-pattern approval needs, and how many needs-repair
//! rounds the agents give one review before it waits for a human. The composition root builds the
//! one [`AgentDecisionPolicy`] of a start from the authoring mode and these settings, and the
//! review console, the scraper's review gate and the spider share it through the one review
//! repository. The console answers the limits at [`AGENT_LIMITS_ROUTE`], so the review agent runner
//! tells the agents the limits the console applies. The operator contract is in
//! `docs/crawler/operations.md` (review agents); the checks an agent approval passes are in
//! [`crate::review::approval_guard`].

use crate::authoring::CrawlerAuthoringMode;
use crate::review::access::{ReviewServerConfigError, positive_number};
use crate::review::model::STATUS_PENDING_REVIEW;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::Duration;

pub const AGENT_MAX_DECISIONS_PER_HOUR_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR";
pub const AGENT_MATRIX_MAX_AGE_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES";
pub const AGENT_MIN_LABELLED_URLS_ENV: &str = "CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS";
pub const AGENT_MAX_REPAIR_ROUNDS_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS";

/// The console's read-only route that answers its [`AgentLimits`].
pub const AGENT_LIMITS_ROUTE: &str = "/api/review-agent-limits";

const DEFAULT_MAX_DECISIONS_PER_HOUR: NonZeroU32 = NonZeroU32::new(20).unwrap();
const DEFAULT_MATRIX_MAX_AGE_MINUTES: NonZeroU32 = NonZeroU32::new(30).unwrap();
const DEFAULT_MIN_LABELLED_URLS: NonZeroU32 = NonZeroU32::new(3).unwrap();
const DEFAULT_MAX_REPAIR_ROUNDS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// The limits of the review agents' own decisions, approvals and rejections alike: the
/// `CRAWLER_REVIEW_AGENT_*` settings, as the console answers them at [`AGENT_LIMITS_ROUTE`]. A
/// limit that is not a whole number of at least 1 is no limit, so it does not deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// The review agents' decision policy of one crawler start, which the composition root builds
/// once from the authoring mode and the [`AgentLimits`] settings and every component shares. Only
/// `agent` mode has review agents. In every other mode no request acts as an agent, so the limits
/// bind nobody, and no review waits for a human, since a human decides every review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentDecisionPolicy {
    authoring: CrawlerAuthoringMode,
    limits: AgentLimits,
}

impl AgentDecisionPolicy {
    pub const fn new(authoring: CrawlerAuthoringMode, limits: AgentLimits) -> Self {
        Self { authoring, limits }
    }

    pub const fn limits(&self) -> &AgentLimits {
        &self.limits
    }

    /// Whether a review in `status` that the agents gave `agent_rounds` needs-repair verdicts
    /// since a human last changed or judged it waits for a human: the review agents work the
    /// reviews, it is pending and their back-and-forth on it reached the cap.
    pub fn waits_for_human(&self, status: &str, agent_rounds: u32) -> bool {
        self.authoring.has_review_agents()
            && status == STATUS_PENDING_REVIEW
            && agent_rounds >= self.limits.max_repair_rounds.get()
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
    fn limits_from(env: &[(&str, &str)]) -> Result<AgentLimits, ReviewServerConfigError> {
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
        let limits = AgentLimits::from_env();
        for name in SETTINGS {
            set(name, None);
        }
        limits
    }

    #[test]
    #[serial_test::serial]
    fn should_take_twenty_decisions_per_hour_and_the_other_defaults_when_nothing_is_set() {
        for env in [
            &[][..],
            &[
                (AGENT_MAX_DECISIONS_PER_HOUR_ENV, " "),
                (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, ""),
                (AGENT_MIN_LABELLED_URLS_ENV, "\t"),
                (AGENT_MAX_REPAIR_ROUNDS_ENV, "  "),
            ][..],
        ] {
            let limits = limits_from(env).unwrap_or_else(|error| panic!("{env:?}: {error}"));

            assert_eq!(limits, AgentLimits::default(), "{env:?}");
            assert_eq!(limits.max_decisions_per_hour.get(), 20);
            assert_eq!(limits.max_matrix_age(), Duration::from_secs(30 * 60));
            assert_eq!(limits.min_labelled_urls.get(), 3);
            assert_eq!(limits.max_repair_rounds.get(), 3);
        }
    }

    #[test]
    #[serial_test::serial]
    fn should_read_the_limits_of_agent_decisions() {
        let limits = limits_from(&[
            (AGENT_MAX_DECISIONS_PER_HOUR_ENV, "5"),
            (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, "10"),
            (AGENT_MIN_LABELLED_URLS_ENV, " 4 "),
            (AGENT_MAX_REPAIR_ROUNDS_ENV, "2"),
        ])
        .unwrap_or_else(|error| panic!("agent decision limits: {error}"));

        assert_eq!(
            limits,
            AgentLimits {
                max_decisions_per_hour: NonZeroU32::new(5).unwrap(),
                matrix_max_age_minutes: NonZeroU32::new(10).unwrap(),
                min_labelled_urls: NonZeroU32::new(4).unwrap(),
                max_repair_rounds: NonZeroU32::new(2).unwrap(),
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
        let error = limits_from(&[(name, value)])
            .err()
            .unwrap_or_else(|| panic!("{name}={value} must fail startup"));

        assert_eq!(error.to_string(), message);
    }

    /// The console answers the limits under the names of their settings; a reader refuses a limit
    /// that is no whole number of at least 1 rather than tell the agents it.
    #[test]
    fn should_answer_the_limits_by_name_and_refuse_one_below_one() {
        let answered = serde_json::to_value(AgentLimits::default()).unwrap();

        assert_eq!(
            answered,
            serde_json::json!({
                "max_decisions_per_hour": 20,
                "matrix_max_age_minutes": 30,
                "min_labelled_urls": 3,
                "max_repair_rounds": 3,
            })
        );
        assert_eq!(
            serde_json::from_value::<AgentLimits>(answered.clone()).unwrap(),
            AgentLimits::default()
        );
        let mut no_labels = answered;
        no_labels["min_labelled_urls"] = serde_json::json!(0);
        assert!(serde_json::from_value::<AgentLimits>(no_labels).is_err());
    }

    fn capped_at_three(authoring: CrawlerAuthoringMode) -> AgentDecisionPolicy {
        AgentDecisionPolicy::new(
            authoring,
            AgentLimits {
                max_repair_rounds: NonZeroU32::new(3).unwrap(),
                ..AgentLimits::default()
            },
        )
    }

    /// The verdict that reaches the cap is still taken; from then on the pending review waits for a
    /// human, and a decided review waits for nobody.
    #[test]
    fn should_hand_a_review_to_a_human_once_the_agents_reached_the_cap() {
        let policy = capped_at_three(CrawlerAuthoringMode::Agent);

        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, 0));
        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, 2));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, 3));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, 4));
        for decided in ["APPROVED", "REJECTED", "SUPERSEDED"] {
            assert!(!policy.waits_for_human(decided, 3), "{decided}");
        }
    }

    /// Outside `agent` mode a human decides every review, so none waits for a human, not even one
    /// that an earlier start's agents sent back and forth up to the cap.
    #[test]
    fn should_hand_no_review_to_a_human_where_no_agent_works_the_reviews() {
        for authoring in [
            CrawlerAuthoringMode::Auto,
            CrawlerAuthoringMode::Review,
            CrawlerAuthoringMode::Manual,
        ] {
            assert!(
                !capped_at_three(authoring).waits_for_human(STATUS_PENDING_REVIEW, 3),
                "{authoring:?}"
            );
        }
        assert!(!AgentDecisionPolicy::default().waits_for_human(STATUS_PENDING_REVIEW, 3));
    }
}
