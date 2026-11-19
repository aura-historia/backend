//! When the review agents try a ListingSource again that they parked: a source labelled by an
//! agent with a label that expires, or one whose qualification an agent left `NEEDS_HUMAN`. Each
//! park of the same source doubles the wait, up to [`MAX_PARK_INTERVAL_DAYS`], so a shop that never
//! works costs little. Human labels and human decisions never expire. The contract is in
//! `docs/crawler/README.md#listingsource-scope` and `#listingsource-qualification`.

use super::SourceLabel;
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use time::{Duration, OffsetDateTime};

/// The longest wait before a parked source is tried again, however often it was parked.
pub const MAX_PARK_INTERVAL_DAYS: u32 = 180;

/// How long a review agent's `NEEDS_HUMAN` qualification stands before the agents try the source
/// again while they still retry it daily: most come from a site that was unreachable for the
/// moment, and several days of failed attempts are what a `SHOP_CLOSED` needs.
pub const AGENT_NEEDS_HUMAN_RETRY_HOURS: i64 = 24;

/// How many daily `NEEDS_HUMAN` qualifications of the review agents a source gets, since a human
/// last decided or cleared its qualification, before the agents give it their final round; every
/// later try is a final round too, on the park schedule.
pub const AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND: u32 = 3;

impl SourceLabel {
    /// Whether the label expires when a review agent set it: the labels that say why the agents
    /// cannot work a real shop for now, which they try again on the park schedule. A human's label
    /// never expires, nor do `ROBOTS_DISALLOWED`, a refused permission that needs contact with the
    /// shop, and the labels a qualification decides and rechecks (`NOT_AN_ESHOP`,
    /// `NOT_AN_ANTIQUE_SHOP`, `SHOP_CLOSED`), `OTHER` and the retired label.
    pub const fn expires_when_agent_set(self) -> bool {
        matches!(
            self,
            Self::BotProtection
                | Self::JavascriptRendered
                | Self::LoginRequired
                | Self::UrlPatternApproachNotApplicable
                | Self::ProductSchemaApproachNotApplicable
                | Self::AgentsUnresolved
        )
    }
}

/// How long the agents leave a parked source before they try it again: `first_interval_days`
/// after the first park, doubling with every further one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkSchedule {
    first_interval_days: NonZeroU32,
}

impl ParkSchedule {
    pub const fn new(first_interval_days: NonZeroU32) -> Self {
        Self {
            first_interval_days,
        }
    }

    /// The wait after a source's `parks`-th agent park label since a human last labelled it or
    /// cleared its label, the one just set included: the first interval, doubled for each park
    /// before it, at most [`MAX_PARK_INTERVAL_DAYS`].
    pub fn label_retry_after(&self, parks: u32) -> Duration {
        Duration::days(self.doubled_days(parks.saturating_sub(1)))
    }

    /// The wait after a source's `needs_human`-th agent `NEEDS_HUMAN` qualification since a human
    /// last decided or cleared it, the one just decided included: a day while the agents retry it
    /// daily and before their final round, then the park schedule from the first interval on.
    pub fn qualification_retry_after(&self, needs_human: u32) -> Duration {
        if needs_human <= AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND {
            return Duration::hours(AGENT_NEEDS_HUMAN_RETRY_HOURS);
        }
        Duration::days(self.doubled_days(needs_human - AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND - 1))
    }

    fn doubled_days(&self, doublings: u32) -> i64 {
        let days = u64::from(self.first_interval_days.get())
            .checked_shl(doublings.min(31))
            .unwrap_or(u64::MAX);
        i64::from(u32::try_from(days.min(u64::from(MAX_PARK_INTERVAL_DAYS))).unwrap_or(0))
    }
}

/// A ListingSource whose agent park label expired and that the agents sent back to qualification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetriedParkedSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    /// The park label that expired, now cleared.
    pub label: SourceLabel,
    #[serde(with = "time::serde::rfc3339")]
    pub labelled_at: OffsetDateTime,
    /// Whether the source waits for a qualification again; `false` when a human qualified it,
    /// which stands, so the next scope refresh enables it at once.
    pub requalifies: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(days: u32) -> ParkSchedule {
        ParkSchedule::new(NonZeroU32::new(days).unwrap_or(NonZeroU32::MIN))
    }

    #[test]
    fn should_expire_only_the_labels_that_park_a_shop_the_agents_cannot_work_for_now() {
        let expiring: Vec<SourceLabel> = SourceLabel::ALL
            .into_iter()
            .filter(|label| label.expires_when_agent_set())
            .collect();
        assert_eq!(
            vec![
                SourceLabel::UrlPatternApproachNotApplicable,
                SourceLabel::ProductSchemaApproachNotApplicable,
                SourceLabel::BotProtection,
                SourceLabel::JavascriptRendered,
                SourceLabel::LoginRequired,
                SourceLabel::AgentsUnresolved,
            ],
            expiring
        );
    }

    #[test]
    fn should_double_the_wait_with_every_park_up_to_the_cap() {
        let schedule = schedule(30);
        let days: Vec<i64> = (1..=5)
            .map(|parks| schedule.label_retry_after(parks).whole_days())
            .collect();
        assert_eq!(vec![30, 60, 120, 180, 180], days);
        assert_eq!(30, schedule.label_retry_after(0).whole_days());
        assert_eq!(180, schedule.label_retry_after(u32::MAX).whole_days());
    }

    #[test]
    fn should_retry_a_needs_human_qualification_daily_then_on_the_park_schedule() {
        let schedule = schedule(30);
        let waits: Vec<Duration> = (1..=6)
            .map(|needs_human| schedule.qualification_retry_after(needs_human))
            .collect();
        assert_eq!(
            vec![
                Duration::hours(24),
                Duration::hours(24),
                Duration::hours(24),
                Duration::days(30),
                Duration::days(60),
                Duration::days(120),
            ],
            waits
        );
    }

    #[test]
    fn should_cap_a_first_interval_above_the_cap() {
        assert_eq!(180, schedule(400).label_retry_after(1).whole_days());
    }
}
