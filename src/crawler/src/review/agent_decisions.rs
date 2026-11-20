//! What the review agents may decide on their own in `agent` authoring mode: how many approvals
//! and rejections per hour, how recent the live matrix a product-schema approval rests on must
//! be, and how many needs-repair rounds the agents give one review before it waits for a human. The composition root builds the
//! one [`AgentDecisionPolicy`] of a start from the authoring mode and these settings, and the
//! review console, the scraper's review gate and the spider share it through the one review
//! repository. The console answers the limits at [`AGENT_LIMITS_ROUTE`], so the review agent runner
//! tells the agents the limits the console applies. The operator contract is in
//! `docs/crawler/operations.md` (review agents); the checks an agent approval passes are in
//! [`crate::review::approval_guard`].

use crate::authoring::CrawlerAuthoringMode;
use crate::review::access::{ReviewServerConfigError, positive_number};
use crate::review::model::{MAX_PARK_INTERVAL_DAYS, ParkSchedule, STATUS_PENDING_REVIEW};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::Duration;

pub const AGENT_MAX_DECISIONS_PER_HOUR_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR";
pub const AGENT_MATRIX_MAX_AGE_MINUTES_ENV: &str = "CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES";
pub const AGENT_MAX_REPAIR_ROUNDS_ENV: &str = "CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS";
pub const AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV: &str =
    "CRAWLER_REVIEW_AGENT_REPAIR_ROUND_WINDOW_DAYS";
pub const AGENT_NO_PATTERN_MAX_AGE_DAYS_ENV: &str = "CRAWLER_REVIEW_AGENT_NO_PATTERN_MAX_AGE_DAYS";
pub const AGENT_PARK_MAX_AGE_DAYS_ENV: &str = "CRAWLER_REVIEW_AGENT_PARK_MAX_AGE_DAYS";

/// The console's read-only route that answers its [`AgentLimits`].
pub const AGENT_LIMITS_ROUTE: &str = "/api/review-agent-limits";

/// About what four agent processes decide in an hour, so the cap stops a runaway rather than
/// throttling the runner's default capacity.
const DEFAULT_MAX_DECISIONS_PER_HOUR: NonZeroU32 = NonZeroU32::new(300).unwrap();
const DEFAULT_MATRIX_MAX_AGE_MINUTES: NonZeroU32 = NonZeroU32::new(30).unwrap();
const DEFAULT_MAX_REPAIR_ROUNDS: NonZeroU32 = NonZeroU32::new(3).unwrap();
const DEFAULT_REPAIR_ROUND_WINDOW_DAYS: NonZeroU32 = NonZeroU32::new(7).unwrap();
const DEFAULT_NO_PATTERN_MAX_AGE_DAYS: NonZeroU32 = NonZeroU32::new(30).unwrap();
const DEFAULT_PARK_MAX_AGE_DAYS: NonZeroU32 = NonZeroU32::new(30).unwrap();

/// The limits of the review agents' own decisions, approvals and rejections alike: the
/// `CRAWLER_REVIEW_AGENT_*` settings, as the console answers them at [`AGENT_LIMITS_ROUTE`]. A
/// limit that is not a whole number of at least 1 is no limit, so it does not deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    /// The agent's approvals plus rejections taken within any rolling hour.
    pub max_decisions_per_hour: NonZeroU32,
    /// How many minutes ago the review pages of the live schema matrix an agent approval rests on
    /// may have been fetched.
    pub matrix_max_age_minutes: NonZeroU32,
    /// How many needs-repair verdicts the agents give a pending review, counted since a human last
    /// changed or judged it and within `repair_round_window_days`, before it waits for a human and
    /// the agents may change or decide nothing on it.
    pub max_repair_rounds: NonZeroU32,
    /// How many days an agent's needs-repair verdict or hand-over to a human counts: a review no
    /// human answered within that time returns to the agents for a fresh attempt, so the reviews
    /// waiting for a human do not pile up with ListingSources nobody looks at.
    pub repair_round_window_days: NonZeroU32,
    /// How many days an agent-approved `NO_PATTERN` holds before the next crawl opens a new
    /// URL-pattern review, so a wrong decision does not remove a shop from product crawling for
    /// good. A human's `NO_PATTERN` holds until a human changes it.
    pub no_pattern_max_age_days: NonZeroU32,
    /// How many days an agent's first park of a ListingSource holds before the agents try it
    /// again: its expiring label or its `NEEDS_HUMAN` after their final round. Every further park
    /// of the same source doubles it ([`ParkSchedule`]).
    pub park_max_age_days: NonZeroU32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_decisions_per_hour: DEFAULT_MAX_DECISIONS_PER_HOUR,
            matrix_max_age_minutes: DEFAULT_MATRIX_MAX_AGE_MINUTES,
            max_repair_rounds: DEFAULT_MAX_REPAIR_ROUNDS,
            repair_round_window_days: DEFAULT_REPAIR_ROUND_WINDOW_DAYS,
            no_pattern_max_age_days: DEFAULT_NO_PATTERN_MAX_AGE_DAYS,
            park_max_age_days: DEFAULT_PARK_MAX_AGE_DAYS,
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
        let limits = Self {
            max_decisions_per_hour: read(
                AGENT_MAX_DECISIONS_PER_HOUR_ENV,
                defaults.max_decisions_per_hour,
            )?,
            matrix_max_age_minutes: read(
                AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
                defaults.matrix_max_age_minutes,
            )?,
            max_repair_rounds: read(AGENT_MAX_REPAIR_ROUNDS_ENV, defaults.max_repair_rounds)?,
            repair_round_window_days: read(
                AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV,
                defaults.repair_round_window_days,
            )?,
            no_pattern_max_age_days: read(
                AGENT_NO_PATTERN_MAX_AGE_DAYS_ENV,
                defaults.no_pattern_max_age_days,
            )?,
            park_max_age_days: read(AGENT_PARK_MAX_AGE_DAYS_ENV, defaults.park_max_age_days)?,
        };
        let first_park_days = limits.park_max_age_days.get().min(MAX_PARK_INTERVAL_DAYS);
        if first_park_days < limits.repair_round_window_days.get() {
            return Err(ReviewServerConfigError::ParkShorterThanRoundWindow {
                park_days: first_park_days,
                window_days: limits.repair_round_window_days.get(),
            });
        }
        Ok(limits)
    }

    /// How long ago the review pages of the live schema matrix an agent approval rests on may have
    /// been fetched.
    pub fn max_matrix_age(&self) -> Duration {
        Duration::from_secs(u64::from(self.matrix_max_age_minutes.get()) * 60)
    }
}

/// What the review agents did on a review since a human last changed or judged it, from its event
/// trail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentRounds {
    /// The agents' needs-repair verdicts: the rounds of their back-and-forth on the review.
    pub needs_repair: u32,
    /// Whether an agent handed the review to a human at once, which needs no further round.
    pub handed_to_human: bool,
    /// Where the agents' final round on the review stands.
    pub final_round: FinalRound,
}

impl AgentRounds {
    pub const fn needs_repair(rounds: u32) -> Self {
        Self {
            needs_repair: rounds,
            handed_to_human: false,
            final_round: FinalRound::NotStarted,
        }
    }
}

/// How many times the agents' final round on a review lets the author agent repair the candidate
/// after a needs-repair verdict in it, so a fix the final-round reviewer names is applied and judged
/// before the review's source is parked: the round ends at the needs-repair verdict that follows
/// them, or at a hand-over (`final_round_of_review` in the review repository compares its count
/// with this value).
pub const FINAL_ROUND_REPAIR_PASSES: u32 = 1;

/// The agents' final round on a review, counted like its rounds since a human last changed or
/// judged it and within the round window: once a review would wait for a human, the author agent
/// may start it once, and the review is the agents' again until a hand-over or a needs-repair
/// verdict beyond its [repair passes](FINAL_ROUND_REPAIR_PASSES) ends it. A review whose final round
/// ended waits for a human until its source is parked; the round counts as one extra round on top
/// of the cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FinalRound {
    #[default]
    NotStarted,
    Open,
    Ended,
}

impl FinalRound {
    /// The state a query reads as `0`, `1` or `2` ([`final_round_of_review`](crate::review::repository)).
    pub const fn from_code(code: i32) -> Self {
        match code {
            0 => Self::NotStarted,
            1 => Self::Open,
            _ => Self::Ended,
        }
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

    /// Whether a ListingSource is crawled only once it is qualified: where the review agents
    /// qualify the sources, as the scope refresh's qualification gate decides
    /// (`docs/crawler/README.md#listingsource-qualification`).
    pub const fn crawls_only_qualified_sources(&self) -> bool {
        self.authoring.has_review_agents()
    }

    /// The agents' needs-repair rounds at which a pending review waits for a human: the
    /// `max_repair_rounds` limit where the review agents work the reviews, and `None` elsewhere,
    /// where no review ever waits for a human. A query that selects reviews by it binds this value.
    pub fn repair_rounds_cap(&self) -> Option<NonZeroU32> {
        self.authoring
            .has_review_agents()
            .then_some(self.limits.max_repair_rounds)
    }

    /// How many days back the agents' needs-repair verdicts and hand-overs count toward a review
    /// waiting for a human, where the review agents work the reviews; `None` elsewhere. A query
    /// that counts the rounds binds this value.
    pub fn repair_round_window_days(&self) -> Option<i32> {
        self.authoring
            .has_review_agents()
            .then(|| i32::try_from(self.limits.repair_round_window_days.get()).unwrap_or(i32::MAX))
    }

    /// How many days an agent-approved `NO_PATTERN` holds before the next crawl opens a new
    /// URL-pattern review, where the review agents work the reviews; `None` elsewhere, where it
    /// holds until changed.
    pub fn no_pattern_max_age_days(&self) -> Option<i32> {
        self.authoring
            .has_review_agents()
            .then(|| i32::try_from(self.limits.no_pattern_max_age_days.get()).unwrap_or(i32::MAX))
    }

    /// When the agents try a ListingSource again that they parked. It binds only review agents'
    /// labels and qualifications, which only `agent` mode has.
    pub fn park_schedule(&self) -> ParkSchedule {
        ParkSchedule::new(self.limits.park_max_age_days)
    }

    /// Whether a review in `status` that the agents worked as `rounds` says since a human last
    /// changed or judged it waits for a human: it is pending and either their back-and-forth on it
    /// reached the [repair-round cap](Self::repair_rounds_cap) or an agent handed it over.
    ///
    /// Once the agents' [final round](FinalRound) started, the cap and the hand-over before it no
    /// longer count: the review waits for a human only once a hand-over, or a needs-repair verdict
    /// beyond the round's [repair passes](FINAL_ROUND_REPAIR_PASSES), ended that round.
    pub fn waits_for_human(&self, status: &str, rounds: AgentRounds) -> bool {
        status == STATUS_PENDING_REVIEW
            && self
                .repair_rounds_cap()
                .is_some_and(|cap| match rounds.final_round {
                    FinalRound::NotStarted => {
                        rounds.handed_to_human || rounds.needs_repair >= cap.get()
                    }
                    FinalRound::Open => false,
                    FinalRound::Ended => true,
                })
    }

    /// Whether the author agent may start the agents' final round on a review in `status` that
    /// the agents worked as `rounds` says: it waits for a human, and no final round started on it
    /// within the round window since a human last acted on it.
    pub fn final_round_due(&self, status: &str, rounds: AgentRounds) -> bool {
        rounds.final_round == FinalRound::NotStarted && self.waits_for_human(status, rounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: [&str; 6] = [
        AGENT_MAX_DECISIONS_PER_HOUR_ENV,
        AGENT_MATRIX_MAX_AGE_MINUTES_ENV,
        AGENT_MAX_REPAIR_ROUNDS_ENV,
        AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV,
        AGENT_NO_PATTERN_MAX_AGE_DAYS_ENV,
        AGENT_PARK_MAX_AGE_DAYS_ENV,
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
    fn should_take_sixty_decisions_per_hour_and_the_other_defaults_when_nothing_is_set() {
        for env in [
            &[][..],
            &[
                (AGENT_MAX_DECISIONS_PER_HOUR_ENV, " "),
                (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, ""),
                (AGENT_MAX_REPAIR_ROUNDS_ENV, "  "),
            ][..],
        ] {
            let limits = limits_from(env).unwrap_or_else(|error| panic!("{env:?}: {error}"));

            assert_eq!(limits, AgentLimits::default(), "{env:?}");
            assert_eq!(limits.max_decisions_per_hour.get(), 300);
            assert_eq!(limits.max_matrix_age(), Duration::from_secs(30 * 60));
            assert_eq!(limits.max_repair_rounds.get(), 3);
            assert_eq!(limits.repair_round_window_days.get(), 7);
            assert_eq!(limits.no_pattern_max_age_days.get(), 30);
            assert_eq!(limits.park_max_age_days.get(), 30);
        }
    }

    #[test]
    #[serial_test::serial]
    fn should_read_the_limits_of_agent_decisions() {
        let limits = limits_from(&[
            (AGENT_MAX_DECISIONS_PER_HOUR_ENV, "5"),
            (AGENT_MATRIX_MAX_AGE_MINUTES_ENV, "10"),
            (AGENT_MAX_REPAIR_ROUNDS_ENV, " 2 "),
            (AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV, "14"),
            (AGENT_NO_PATTERN_MAX_AGE_DAYS_ENV, "60"),
            (AGENT_PARK_MAX_AGE_DAYS_ENV, "45"),
        ])
        .unwrap_or_else(|error| panic!("agent decision limits: {error}"));

        assert_eq!(
            limits,
            AgentLimits {
                max_decisions_per_hour: NonZeroU32::new(5).unwrap(),
                matrix_max_age_minutes: NonZeroU32::new(10).unwrap(),
                max_repair_rounds: NonZeroU32::new(2).unwrap(),
                repair_round_window_days: NonZeroU32::new(14).unwrap(),
                no_pattern_max_age_days: NonZeroU32::new(60).unwrap(),
                park_max_age_days: NonZeroU32::new(45).unwrap(),
            }
        );
    }

    #[test]
    #[serial_test::serial]
    fn should_refuse_a_park_shorter_than_the_round_window() {
        for (window, park, first_park) in [("7", "3", 3), ("200", "200", 180)] {
            let error = limits_from(&[
                (AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV, window),
                (AGENT_PARK_MAX_AGE_DAYS_ENV, park),
            ])
            .err()
            .unwrap_or_else(|| panic!("a {park}-day park under a {window}-day window"));

            assert!(
                matches!(
                    error,
                    ReviewServerConfigError::ParkShorterThanRoundWindow { park_days, .. }
                        if park_days == first_park
                ),
                "{error}"
            );
        }
        assert!(
            limits_from(&[
                (AGENT_REPAIR_ROUND_WINDOW_DAYS_ENV, "180"),
                (AGENT_PARK_MAX_AGE_DAYS_ENV, "400"),
            ])
            .is_ok()
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
                "max_decisions_per_hour": 300,
                "matrix_max_age_minutes": 30,
                "max_repair_rounds": 3,
                "repair_round_window_days": 7,
                "no_pattern_max_age_days": 30,
                "park_max_age_days": 30,
            })
        );
        assert_eq!(
            serde_json::from_value::<AgentLimits>(answered.clone()).unwrap(),
            AgentLimits::default()
        );
        let mut no_rounds = answered;
        no_rounds["max_repair_rounds"] = serde_json::json!(0);
        assert!(serde_json::from_value::<AgentLimits>(no_rounds).is_err());
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

    /// The round window and the `NO_PATTERN` age bind only where the review agents work the
    /// reviews: elsewhere a human decides every review and nothing expires.
    #[test]
    fn should_bind_the_round_window_and_no_pattern_age_only_in_agent_mode() {
        let agent = AgentDecisionPolicy::new(CrawlerAuthoringMode::Agent, AgentLimits::default());
        let manual = AgentDecisionPolicy::new(CrawlerAuthoringMode::Manual, AgentLimits::default());

        assert_eq!(
            (
                agent.repair_round_window_days(),
                agent.no_pattern_max_age_days()
            ),
            (Some(7), Some(30))
        );
        assert_eq!(
            (
                manual.repair_round_window_days(),
                manual.no_pattern_max_age_days()
            ),
            (None, None)
        );
    }

    /// The verdict that reaches the cap is still taken; from then on the pending review waits for a
    /// human, and a decided review waits for nobody.
    #[test]
    fn should_hand_a_review_to_a_human_once_the_agents_reached_the_cap() {
        let policy = capped_at_three(CrawlerAuthoringMode::Agent);

        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(0)));
        assert!(!policy.waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(2)));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(3)));
        assert!(policy.waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(4)));
        for decided in ["APPROVED", "REJECTED", "SUPERSEDED"] {
            assert!(
                !policy.waits_for_human(decided, AgentRounds::needs_repair(3)),
                "{decided}"
            );
        }
    }

    /// An agent's hand-off needs no round, but only where review agents work the reviews and
    /// only on a pending review.
    #[test]
    fn should_hand_a_review_to_a_human_when_an_agent_handed_it_over_before_the_cap() {
        let handed = AgentRounds {
            needs_repair: 1,
            handed_to_human: true,
            final_round: FinalRound::NotStarted,
        };
        let agent = capped_at_three(CrawlerAuthoringMode::Agent);

        assert!(agent.waits_for_human(STATUS_PENDING_REVIEW, handed));
        assert!(!agent.waits_for_human("APPROVED", handed));
        assert!(
            !capped_at_three(CrawlerAuthoringMode::Review)
                .waits_for_human(STATUS_PENDING_REVIEW, handed)
        );
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
                !capped_at_three(authoring)
                    .waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(3)),
                "{authoring:?}"
            );
        }
        assert!(
            !AgentDecisionPolicy::default()
                .waits_for_human(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(3))
        );
        assert_eq!(AgentDecisionPolicy::default().repair_rounds_cap(), None);
        assert_eq!(
            capped_at_three(CrawlerAuthoringMode::Agent).repair_rounds_cap(),
            NonZeroU32::new(3)
        );
    }

    /// The final round lifts the cap and the hand-over once: the review is the agents' while it is
    /// open, waits for a human once it ended, and is due only before it started.
    #[test]
    fn should_give_a_review_that_waits_for_a_human_one_final_round() {
        let policy = capped_at_three(CrawlerAuthoringMode::Agent);
        let at_cap = |final_round| AgentRounds {
            needs_repair: 3,
            handed_to_human: false,
            final_round,
        };
        let handed = |final_round| AgentRounds {
            needs_repair: 0,
            handed_to_human: true,
            final_round,
        };

        for rounds in [
            at_cap(FinalRound::NotStarted),
            handed(FinalRound::NotStarted),
        ] {
            assert!(
                policy.waits_for_human(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
            assert!(
                policy.final_round_due(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
        }
        for rounds in [at_cap(FinalRound::Open), handed(FinalRound::Open)] {
            assert!(
                !policy.waits_for_human(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
            assert!(
                !policy.final_round_due(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
        }
        for rounds in [at_cap(FinalRound::Ended), handed(FinalRound::Ended)] {
            assert!(
                policy.waits_for_human(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
            assert!(
                !policy.final_round_due(STATUS_PENDING_REVIEW, rounds),
                "{rounds:?}"
            );
        }
        assert!(!policy.final_round_due(STATUS_PENDING_REVIEW, AgentRounds::needs_repair(2)));
        assert!(!policy.final_round_due("APPROVED", at_cap(FinalRound::NotStarted)));
        assert!(
            !capped_at_three(CrawlerAuthoringMode::Manual)
                .final_round_due(STATUS_PENDING_REVIEW, at_cap(FinalRound::NotStarted))
        );
    }
}
