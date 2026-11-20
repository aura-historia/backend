//! The identity conflicts in the log: while product URLs of crawl-enabled ListingSources are in
//! `SourceListingIdConflict`, the console warns on every report with how many sources and URLs
//! are stuck, the most conflicted sources and since when, so a conflict nothing waits on still
//! reaches an operator without the console. A URL keeps its captured identity on its own page, so
//! a conflict is a redirect to a page with another identity, or a page that shows the identity of
//! another known product (`docs/crawler/scheduling-and-retries.md#identity-conflicts`). It is a log
//! event and no metric, because every metric is billed. The event is in
//! `docs/crawler/operations.md` (outcome events).

use crate::review::readers::{IdentityConflicts, ListingSourceOverviewReader};
use std::convert::Infallible;
use std::time::Duration;
use tracing::{debug, warn};

/// How often the console reports the identity conflicts.
const REPORT_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// The most sources one report names; the totals count every one.
const REPORTED_SOURCES: i64 = 10;
const IDENTITY_CONFLICTS_EVENT: &str = "crawler.identity_conflicts";

/// Reports the identity conflicts `sources` reads every [`REPORT_INTERVAL`], from the first tick
/// on. Conflicts that cannot be read are warned about and reported at the next tick.
pub(super) async fn report_while_conflicts_last(
    sources: ListingSourceOverviewReader,
) -> Infallible {
    let mut ticks = tokio::time::interval(REPORT_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        match sources.identity_conflicts(REPORTED_SOURCES).await {
            Ok(conflicts) => log(&conflicts),
            Err(error) => warn!(
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Cannot read the identity conflicts; the next report reads them again"
            ),
        }
    }
}

/// No conflict is a heartbeat: WARN carries only conflicts an operator has to resolve.
fn log(conflicts: &IdentityConflicts) {
    if conflicts.sources_total == 0 {
        debug!(
            event = IDENTITY_CONFLICTS_EVENT,
            sources = 0,
            urls = 0,
            "No product URL is in an identity conflict"
        );
        return;
    }
    warn!(
        event = IDENTITY_CONFLICTS_EVENT,
        sources = conflicts.sources_total,
        urls = conflicts.urls_total,
        conflicted_sources = %reported_sources(conflicts),
        "Product URLs redirect to a page with another identity or show another known product and are not captured; check the pages"
    );
}

/// The reported sources as one JSON array of `listing_source_id`, `listing_source_name`,
/// `crawl_host`, `urls` and `since`.
fn reported_sources(conflicts: &IdentityConflicts) -> String {
    serde_json::to_string(&conflicts.sources).unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::readers::IdentityConflictedSource;
    use listing_source_core::ListingSourceId;
    use time::macros::datetime;

    #[test]
    fn should_name_each_reported_source_with_its_urls_and_since_when() {
        let listing_source_id = ListingSourceId::new();
        let conflicts = IdentityConflicts {
            sources_total: 26,
            urls_total: 1601,
            sources: vec![IdentityConflictedSource {
                listing_source_id,
                listing_source_name: Some("HaresFur".to_owned()),
                crawl_host: Some("haresfur.example".to_owned()),
                urls: 545,
                since: Some(datetime!(2026-11-09 08:00 UTC)),
            }],
        };

        let reported: serde_json::Value =
            serde_json::from_str(&reported_sources(&conflicts)).unwrap();

        assert_eq!(
            reported,
            serde_json::json!([{
                "listing_source_id": listing_source_id,
                "listing_source_name": "HaresFur",
                "crawl_host": "haresfur.example",
                "urls": 545,
                "since": "2026-11-09T08:00:00Z",
            }])
        );
    }
}
