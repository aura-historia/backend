//! `GET /api/source-events`: what the crawler did to ListingSources lately. The contract is in
//! `docs/crawler/operations.md` (review service).

use listing_source_core::ListingSourceId;
use serde::Serialize;
use time::OffsetDateTime;

/// What happened to a ListingSource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceEventKind {
    /// The source's current streak of same-kind scrape failures began.
    ScrapeFailing,
    /// The spider's latest run on the source ended, with its outcome.
    CrawlFinished,
}

/// One thing that happened to a ListingSource, dated by the time the crawler recorded for it on the
/// source. The crawler keeps no history of source state: an event is the source's current state
/// when it is dated within the window, so a failure streak that ended, or a crawl that a later one
/// replaced, is not listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceEvent {
    pub kind: SourceEventKind,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred: OffsetDateTime,
    /// The crawl outcome of `CRAWL_FINISHED`.
    pub outcome: Option<String>,
    /// The error kind of the failures of `SCRAPE_FAILING`, or of the crawl that finished.
    pub error_kind: Option<String>,
    /// How many scrapes failed in a row (`SCRAPE_FAILING`), or crawls in a row (`CRAWL_FINISHED`).
    pub count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_serialize_an_event_as_the_contract_names_it() {
        let event = SourceEvent {
            kind: SourceEventKind::ScrapeFailing,
            listing_source_id: ListingSourceId::new(),
            listing_source_name: Some("Antiques".to_owned()),
            occurred: OffsetDateTime::UNIX_EPOCH,
            outcome: None,
            error_kind: Some("TIMEOUT".to_owned()),
            count: 14,
        };

        let json = serde_json::to_value(event).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(json["kind"], "SCRAPE_FAILING");
        assert_eq!(json["occurred"], "1970-01-01T00:00:00Z");
        assert_eq!(json["count"], json!(14));
        assert_eq!(json["outcome"], json!(null));
    }
}
