//! Mass-removal guard of a scraper domain batch: a batch that would capture the removal of most
//! of the URLs it scraped captures none of them.
//!
//! The rule is pure. The scraper cron counts a batch's new removals and scraped URLs, asks for
//! the decision once the batch has scraped its last URL and captures or holds the removals;
//! `docs/crawler/scheduling-and-retries.md` (mass-removal guard) owns the durable contract.

/// New removals one batch must reach before their share is judged, so a shop with few due URLs
/// is never held back by its own few removals.
pub const MASS_REMOVAL_MIN_URLS: usize = 10;
/// The share of a batch's scraped URLs, in percent, that its new removals must exceed to be
/// held: more than half, so the guard holds a batch that would remove most of what it checked.
pub const MASS_REMOVAL_MAX_SHARE_PERCENT: usize = 50;
/// `last_error_kind` the scraper records for a URL whose removal the guard held back, which the
/// ListingSource overview counts as held removals.
pub const MASS_REMOVAL_HELD_ERROR_KIND: &str = "MassRemovalHeld";

/// The new removals of a batch that the guard holds back: at least [`MASS_REMOVAL_MIN_URLS`]
/// and more than [`MASS_REMOVAL_MAX_SHARE_PERCENT`] percent of the URLs it scraped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MassRemoval {
    removals: usize,
    scraped_urls: usize,
}

impl MassRemoval {
    /// The mass removal `removals` new removals among `scraped_urls` scraped URLs are, or `None`
    /// when the batch may capture them. `scraped_urls` counts the URLs the batch fetched, not the
    /// rechecks of pages whose removal is already captured.
    pub fn of(removals: usize, scraped_urls: usize) -> Option<Self> {
        let scraped_urls = scraped_urls.max(removals);
        (removals >= MASS_REMOVAL_MIN_URLS
            && removals * 100 > scraped_urls * MASS_REMOVAL_MAX_SHARE_PERCENT)
            .then_some(Self {
                removals,
                scraped_urls,
            })
    }

    pub fn removals(self) -> usize {
        self.removals
    }

    pub fn scraped_urls(self) -> usize {
        self.scraped_urls
    }

    /// The removals' share of the scraped URLs, from 0 to 1.
    pub fn share(self) -> f64 {
        self.removals as f64 / self.scraped_urls as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_hold_a_batch_that_would_remove_every_url_it_scraped() {
        let mass = MassRemoval::of(50, 50).expect("every scraped URL removed");

        assert_eq!((mass.removals(), mass.scraped_urls()), (50, 50));
        assert_eq!(mass.share(), 1.0);
    }

    #[test]
    fn should_hold_a_batch_whose_removals_are_just_more_than_half() {
        assert!(MassRemoval::of(11, 21).is_some());
    }

    #[test]
    fn should_capture_removals_that_are_half_of_the_batch_or_less() {
        assert_eq!(MassRemoval::of(10, 20), None);
        assert_eq!(MassRemoval::of(30, 100), None);
    }

    #[test]
    fn should_capture_fewer_removals_than_the_minimum_whatever_their_share() {
        assert_eq!(MassRemoval::of(MASS_REMOVAL_MIN_URLS - 1, 9), None);
        assert_eq!(MassRemoval::of(1, 1), None);
    }

    #[test]
    fn should_judge_removals_against_at_least_themselves() {
        let mass = MassRemoval::of(12, 0).expect("removals are scraped URLs too");

        assert_eq!(mass.scraped_urls(), 12);
    }
}
